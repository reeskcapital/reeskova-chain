//! Validación RSC-WASM-V1.
//!
//! Un `WasmModule` solo existe si el binario pasó el perfil. No se normaliza
//! ni se reescribe el bytecode: el id es el SHA-256 de esos bytes.

use crate::ExecutionError;
use rsc_crypto::hash;
use wasmparser::{Encoding, Parser, Payload, ValType, Validator, WasmFeatures};

/// Perfil de validación de RSC-WASM-V1.
///
/// Se construye desde `empty()` y solo se encienden capabilities enteras
/// aprobadas. Nunca se usa `WasmFeatures::default()`.
pub fn rsc_wasm_v1_features() -> WasmFeatures {
    let mut f = WasmFeatures::empty();
    f.insert(WasmFeatures::MUTABLE_GLOBAL);
    f.insert(WasmFeatures::SIGN_EXTENSION);
    f.insert(WasmFeatures::MULTI_VALUE);
    f.insert(WasmFeatures::BULK_MEMORY);
    f
}

/// Límites protocolarios. Deben ser idénticos entre validators.
///
/// Los límites operacionales del host (p. ej. reserva de VA) no pueden
/// cambiar el resultado protocolario.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmLimits {
    pub max_module_size: usize,
    pub max_memories: u32,
    pub max_memory_pages: u32,
    pub max_tables: u32,
    pub max_functions: u32,
    pub max_globals: u32,
    pub max_imports: u32,
    pub max_exports: u32,
}

impl Default for WasmLimits {
    fn default() -> Self {
        Self {
            max_module_size: 1_048_576,
            max_memories: 1,
            max_memory_pages: 16,
            max_tables: 0,
            max_functions: 256,
            max_globals: 64,
            max_imports: 0,
            max_exports: 64,
        }
    }
}

/// Identidad determinista: SHA-256 de los bytes aceptados.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WasmModuleId([u8; 32]);

impl WasmModuleId {
    pub fn from_module_bytes(bytes: &[u8]) -> Self {
        Self(hash(bytes).to_bytes())
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl core::fmt::Debug for WasmModuleId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "WasmModuleId({:02x}{:02x}..)", self.0[0], self.0[1])
    }
}

/// Módulo core ya validado contra RSC-WASM-V1.
#[derive(Clone, Debug)]
pub struct WasmModule {
    bytes: Vec<u8>,
    id: WasmModuleId,
}

impl WasmModule {
    /// Valida `bytes` y, si el perfil y los límites se cumplen, produce el módulo.
    pub fn validate(bytes: &[u8], limits: &WasmLimits) -> Result<Self, ExecutionError> {
        validate_module(bytes, limits)?;
        Ok(Self {
            id: WasmModuleId::from_module_bytes(bytes),
            bytes: bytes.to_vec(),
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn id(&self) -> WasmModuleId {
        self.id
    }
}

fn validate_module(bytes: &[u8], limits: &WasmLimits) -> Result<(), ExecutionError> {
    if bytes.is_empty() {
        return Err(ExecutionError::InvalidModule);
    }
    if bytes.len() > limits.max_module_size {
        return Err(ExecutionError::ModuleTooLarge);
    }
    if is_component_header(bytes) {
        return Err(ExecutionError::ComponentNotSupported);
    }

    let features = rsc_wasm_v1_features();
    let mut parser = Parser::new(0);
    parser.set_features(features);
    let mut validator = Validator::new_with_features(features);

    let mut memories = 0u32;
    let mut tables = 0u32;
    let mut functions = 0u32;
    let mut globals = 0u32;
    let mut imports = 0u32;
    let mut exports = 0u32;

    for payload in parser.parse_all(bytes) {
        let payload = payload.map_err(map_parser_error)?;
        match &payload {
            Payload::Version { encoding, .. } => {
                if *encoding == Encoding::Component {
                    return Err(ExecutionError::ComponentNotSupported);
                }
            }
            Payload::ImportSection(reader) => {
                imports = imports.saturating_add(reader.count());
                if imports > limits.max_imports || imports > 0 {
                    return Err(ExecutionError::UnsupportedImport);
                }
            }
            Payload::ExportSection(reader) => {
                exports = exports.saturating_add(reader.count());
                if exports > limits.max_exports {
                    return Err(ExecutionError::TooManyExports);
                }
            }
            Payload::FunctionSection(reader) => {
                functions = functions.saturating_add(reader.count());
                if functions > limits.max_functions {
                    return Err(ExecutionError::TooManyFunctions);
                }
            }
            Payload::GlobalSection(reader) => {
                for global in reader.clone() {
                    let global = global.map_err(|_| ExecutionError::InvalidModule)?;
                    if !matches!(global.ty.content_type, ValType::I32 | ValType::I64) {
                        return Err(ExecutionError::UnsupportedFeature);
                    }
                    globals = globals.saturating_add(1);
                    if globals > limits.max_globals {
                        return Err(ExecutionError::TooManyGlobals);
                    }
                }
            }
            Payload::MemorySection(reader) => {
                for memory in reader.clone() {
                    let memory = memory.map_err(|_| ExecutionError::InvalidModule)?;
                    if memory.memory64 {
                        return Err(ExecutionError::UnsupportedFeature);
                    }
                    if memory.shared {
                        return Err(ExecutionError::UnsupportedFeature);
                    }
                    if memory.initial > u64::from(limits.max_memory_pages) {
                        return Err(ExecutionError::MemoryLimitExceeded);
                    }
                    if let Some(max) = memory.maximum {
                        if max > u64::from(limits.max_memory_pages) {
                            return Err(ExecutionError::MemoryLimitExceeded);
                        }
                    }
                    memories = memories.saturating_add(1);
                    if memories > limits.max_memories {
                        return Err(ExecutionError::TooManyMemories);
                    }
                }
            }
            Payload::TableSection(reader) => {
                tables = tables.saturating_add(reader.count());
                if tables > limits.max_tables {
                    return Err(ExecutionError::TooManyTables);
                }
            }
            Payload::StartSection { .. } => {
                return Err(ExecutionError::StartFunctionNotSupported);
            }
            _ => {}
        }
        validator.payload(&payload).map_err(map_parser_error)?;
    }
    Ok(())
}

/// Core module = magic `\0asm` + version 1. Cualquier otra versión con magic
/// WASM se trata como component / encoding no soportado.
fn is_component_header(bytes: &[u8]) -> bool {
    if bytes.len() < 8 || bytes[0..4] != *b"\0asm" {
        return false;
    }
    let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    version != 1
}

fn map_parser_error(err: wasmparser::BinaryReaderError) -> ExecutionError {
    let msg = err.message().to_ascii_lowercase();
    if msg.contains("component") {
        ExecutionError::ComponentNotSupported
    } else if msg.contains("import") {
        ExecutionError::UnsupportedImport
    } else if contains_any(
        &msg,
        &[
            "float",
            "f32",
            "f64",
            "simd",
            "v128",
            "thread",
            "shared",
            "atomic",
            "memory64",
            "reference",
            "funcref",
            "externref",
            "gc",
            "exception",
            "tag",
            "multi-memory",
            "multiple memories",
        ],
    ) {
        ExecutionError::UnsupportedFeature
    } else {
        ExecutionError::InvalidModule
    }
}

fn contains_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

#[cfg(test)]
pub(crate) fn wat_bytes(src: &str) -> Vec<u8> {
    wat::parse_str(src).unwrap_or_else(|e| panic!("test wat: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> WasmModule {
        WasmModule::validate(&wat_bytes(src), &WasmLimits::default()).expect("valid module")
    }

    fn err(src: &str) -> ExecutionError {
        WasmModule::validate(&wat_bytes(src), &WasmLimits::default()).expect_err("rejected")
    }

    fn err_bytes(bytes: &[u8]) -> ExecutionError {
        WasmModule::validate(bytes, &WasmLimits::default()).expect_err("rejected")
    }

    #[test]
    fn test_valid_core_module() {
        let m = ok("(module (func (export \"n\")))");
        assert!(!m.bytes().is_empty());
    }

    #[test]
    fn test_invalid_binary_rejected() {
        assert_eq!(err_bytes(b""), ExecutionError::InvalidModule);
        assert_eq!(err_bytes(b"not wasm"), ExecutionError::InvalidModule);
        assert_eq!(err_bytes(b"\0asm\x01\x00"), ExecutionError::InvalidModule);
        assert_eq!(
            err_bytes(&[0, 1, 2, 3, 4, 5, 6, 7, 8]),
            ExecutionError::InvalidModule
        );
    }

    #[test]
    fn test_component_rejected() {
        let bytes = wat_bytes("(component)");
        assert_eq!(
            WasmModule::validate(&bytes, &WasmLimits::default()).unwrap_err(),
            ExecutionError::ComponentNotSupported
        );
    }

    #[test]
    fn test_float_module_rejected() {
        assert_eq!(
            err("(module (func (export \"f\") (result f32) f32.const 1.0))"),
            ExecutionError::UnsupportedFeature
        );
        assert_eq!(
            err("(module (func (export \"f\") (result f64) f64.const 1.0))"),
            ExecutionError::UnsupportedFeature
        );
    }

    #[test]
    fn test_simd_module_rejected() {
        assert_eq!(
            err("(module (func (export \"s\") (result v128) v128.const i32x4 0 0 0 0))"),
            ExecutionError::UnsupportedFeature
        );
    }

    #[test]
    fn test_relaxed_simd_rejected() {
        assert_eq!(
            err(r#"(module
                    (func (export "r") (param v128) (result v128)
                      local.get 0
                      i32x4.relaxed_trunc_f32x4_s))"#,),
            ExecutionError::UnsupportedFeature
        );
    }

    #[test]
    fn test_threads_rejected() {
        assert_eq!(
            err("(module (memory 1 1 shared))"),
            ExecutionError::UnsupportedFeature
        );
    }

    #[test]
    fn test_reference_types_rejected() {
        assert_eq!(
            err("(module (func (export \"r\") (param externref)))"),
            ExecutionError::UnsupportedFeature
        );
    }

    #[test]
    fn test_memory64_rejected() {
        assert_eq!(
            err("(module (memory i64 1))"),
            ExecutionError::UnsupportedFeature
        );
    }

    #[test]
    fn test_multi_memory_rejected() {
        assert_eq!(
            err("(module (memory 1) (memory 1))"),
            ExecutionError::TooManyMemories
        );
    }

    #[test]
    fn test_gc_rejected() {
        assert_eq!(
            err("(module (type $s (struct (field i32))))"),
            ExecutionError::UnsupportedFeature
        );
    }

    #[test]
    fn test_exceptions_rejected() {
        assert_eq!(
            err("(module (tag $e (param i32)))"),
            ExecutionError::UnsupportedFeature
        );
    }

    #[test]
    fn test_unsupported_import_rejected() {
        assert_eq!(
            err("(module (import \"env\" \"random\" (func (result i32))))"),
            ExecutionError::UnsupportedImport
        );
    }

    #[test]
    fn test_start_function_rejected() {
        assert_eq!(
            err("(module (func $s) (start $s))"),
            ExecutionError::StartFunctionNotSupported
        );
    }

    #[test]
    fn test_oversized_module_rejected() {
        let bytes = wat_bytes("(module (func (export \"n\")))");
        let limits = WasmLimits {
            max_module_size: 8,
            ..WasmLimits::default()
        };
        assert_eq!(
            WasmModule::validate(&bytes, &limits).unwrap_err(),
            ExecutionError::ModuleTooLarge
        );
    }

    #[test]
    fn test_max_module_size() {
        let bytes = wat_bytes("(module (func (export \"n\")))");
        let ok_limits = WasmLimits {
            max_module_size: bytes.len(),
            ..WasmLimits::default()
        };
        assert!(WasmModule::validate(&bytes, &ok_limits).is_ok());
        let over = WasmLimits {
            max_module_size: bytes.len() - 1,
            ..WasmLimits::default()
        };
        assert_eq!(
            WasmModule::validate(&bytes, &over).unwrap_err(),
            ExecutionError::ModuleTooLarge
        );
    }

    #[test]
    fn test_memory_limit() {
        assert!(!ok("(module (memory 1) (func (export \"n\")))")
            .bytes()
            .is_empty());
        assert_eq!(
            err("(module (memory 32) (func (export \"n\")))"),
            ExecutionError::MemoryLimitExceeded
        );
    }

    #[test]
    fn test_function_limit() {
        let mut src = String::from("(module");
        for _ in 0..257 {
            src.push_str(" (func)");
        }
        src.push(')');
        assert_eq!(err(&src), ExecutionError::TooManyFunctions);
    }

    #[test]
    fn test_global_limit() {
        let mut src = String::from("(module");
        for _ in 0..65 {
            src.push_str(" (global i32 i32.const 0)");
        }
        src.push(')');
        assert_eq!(err(&src), ExecutionError::TooManyGlobals);
    }

    #[test]
    fn test_export_limit() {
        let mut src = String::from("(module");
        for i in 0..65 {
            src.push_str(&format!(" (func (export \"e{i}\"))"));
        }
        src.push(')');
        assert_eq!(err(&src), ExecutionError::TooManyExports);
    }

    #[test]
    fn test_import_limit() {
        assert_eq!(
            err("(module (import \"env\" \"time\" (func (result i32))))"),
            ExecutionError::UnsupportedImport
        );
        assert_eq!(
            err("(module (import \"env\" \"network\" (func)))"),
            ExecutionError::UnsupportedImport
        );
        assert_eq!(
            err("(module (import \"env\" \"storage\" (func)))"),
            ExecutionError::UnsupportedImport
        );
        assert_eq!(
            err("(module (import \"env\" \"fs\" (func)))"),
            ExecutionError::UnsupportedImport
        );
    }

    #[test]
    fn test_module_id_deterministic() {
        let a = ok("(module (func (export \"n\")))");
        let b = ok("(module (func (export \"n\")))");
        assert_eq!(a.id(), b.id());
    }

    #[test]
    fn test_same_bytes_same_id() {
        let bytes = wat_bytes("(module (func (export \"n\")))");
        let a = WasmModule::validate(&bytes, &WasmLimits::default()).unwrap();
        let b = WasmModule::validate(&bytes, &WasmLimits::default()).unwrap();
        assert_eq!(a.id(), b.id());
        assert_eq!(a.id(), WasmModuleId::from_module_bytes(&bytes));
    }

    #[test]
    fn test_different_bytes_different_id() {
        let a = ok("(module (func (export \"a\")))");
        let b = ok("(module (func (export \"b\")))");
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn test_malformed_module_no_panic() {
        for bytes in [
            &b""[..],
            &b"\0asm"[..],
            &[0xff; 64],
            b"\0asm\x01\x00\x00\x00\xff\xff",
        ] {
            let _ = WasmModule::validate(bytes, &WasmLimits::default());
        }
    }

    #[test]
    fn test_no_wasi() {
        assert_eq!(
            err("(module (import \"wasi_snapshot_preview1\" \"random_get\" (func (param i32 i32) (result i32))))"),
            ExecutionError::UnsupportedImport
        );
    }

    #[test]
    fn test_no_imports() {
        assert_eq!(
            err("(module (import \"env\" \"x\" (func)))"),
            ExecutionError::UnsupportedImport
        );
    }

    #[test]
    fn test_no_start_function() {
        assert_eq!(
            err("(module (func $s) (start $s) (func (export \"n\")))"),
            ExecutionError::StartFunctionNotSupported
        );
    }

    #[test]
    fn test_no_tables() {
        assert_eq!(
            err("(module (table 1 funcref))"),
            ExecutionError::TooManyTables
        );
    }
}
