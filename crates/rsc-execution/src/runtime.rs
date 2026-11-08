//! Runtime Wasmtime 48.0.2 para RSC-WASM-V1.
//!
//! Un Store por ejecución. Fuel determinista. Sin WASI, sin host functions,
//! sin reloj. Los costes de operador son los de Wasmtime 48.0.2 y forman
//! parte del entorno.

use crate::wasm::{WasmLimits, WasmModule, WasmModuleId};
use crate::{ExecutionError, EXECUTION_ENVIRONMENT_ID};
use wasmparser::WasmFeatures;
use wasmtime::{Config, Engine, Extern, Instance, Module, Store, Val, ValType};

/// Configuración del engine. Inmutable una vez creado [`ExecutionEngine`].
///
/// `memory_limit_pages` y `module_limits` son límites protocolarios.
/// No mezclar aquí HTTP, consenso ni storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionConfig {
    pub fuel_limit: u64,
    pub memory_limit_pages: u32,
    pub max_output_size: usize,
    pub module_limits: WasmLimits,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        let module_limits = WasmLimits::default();
        Self {
            fuel_limit: 1_000_000,
            memory_limit_pages: module_limits.max_memory_pages,
            max_output_size: 4096,
            module_limits,
        }
    }
}

impl ExecutionConfig {
    pub fn environment_id(&self) -> &'static str {
        EXECUTION_ENVIRONMENT_ID
    }
}

/// Valores permitidos en V1. Sin floats ni referencias.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WasmValue {
    I32(i32),
    I64(i64),
}

impl WasmValue {
    fn encoded_size(self) -> usize {
        match self {
            Self::I32(_) => 4,
            Self::I64(_) => 8,
        }
    }

    fn to_val(self) -> Val {
        match self {
            Self::I32(v) => Val::I32(v),
            Self::I64(v) => Val::I64(v),
        }
    }

    fn from_val(val: &Val) -> Result<Self, ExecutionError> {
        match val {
            Val::I32(v) => Ok(Self::I32(*v)),
            Val::I64(v) => Ok(Self::I64(*v)),
            _ => Err(ExecutionError::InvalidFunctionSignature),
        }
    }
}

/// Pedido de ejecución de una exportación concreta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionRequest {
    pub function: String,
    pub arguments: Vec<WasmValue>,
    pub fuel_limit: u64,
}

/// Resultado protocolario. Sin wall-clock, CPU, OS ni direcciones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionResult {
    pub return_values: Vec<WasmValue>,
    pub fuel_used: u64,
    pub module_id: WasmModuleId,
}

/// Módulo compilado. El JIT nativo no entra en el consenso.
pub struct CompiledModule {
    module: Module,
    id: WasmModuleId,
}

/// Motor de ejecución. `Engine` de Wasmtime compartible; Store de vida corta.
pub struct ExecutionEngine {
    engine: Engine,
    config: ExecutionConfig,
}

struct GuestLimits {
    max_memory_bytes: usize,
}

impl wasmtime::ResourceLimiter for GuestLimits {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        if desired > self.max_memory_bytes {
            return Err(wasmtime::Error::msg("memory limit exceeded"));
        }
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        Ok(false)
    }
}

impl ExecutionEngine {
    pub fn new(config: ExecutionConfig) -> Result<Self, ExecutionError> {
        if config.fuel_limit == 0 || config.memory_limit_pages == 0 {
            return Err(ExecutionError::InvalidConfiguration);
        }
        let engine = Engine::new(&wasmtime_config(&config))
            .map_err(|_| ExecutionError::EngineInitializationFailed)?;
        Ok(Self { engine, config })
    }

    pub fn config(&self) -> &ExecutionConfig {
        &self.config
    }

    /// Compila un módulo ya validado. No acepta bytes crudos.
    pub fn compile(&self, module: &WasmModule) -> Result<CompiledModule, ExecutionError> {
        let compiled = Module::new(&self.engine, module.bytes())
            .map_err(|_| ExecutionError::CompilationFailed)?;
        Ok(CompiledModule {
            module: compiled,
            id: module.id(),
        })
    }

    pub fn execute(
        &self,
        module: &CompiledModule,
        request: &ExecutionRequest,
    ) -> Result<ExecutionResult, ExecutionError> {
        if request.fuel_limit == 0 {
            return Err(ExecutionError::FuelExhausted);
        }
        if request.fuel_limit > self.config.fuel_limit {
            return Err(ExecutionError::InvalidConfiguration);
        }

        let max_memory_bytes = (self.config.memory_limit_pages as usize).saturating_mul(65536);
        let mut store = Store::new(&self.engine, GuestLimits { max_memory_bytes });
        store.limiter(|s| s);
        store
            .set_fuel(request.fuel_limit)
            .map_err(|_| ExecutionError::InvalidConfiguration)?;

        let instance =
            Instance::new(&mut store, &module.module, &[]).map_err(|e| classify_trap(&store, e))?;

        let export = instance
            .get_export(&mut store, &request.function)
            .ok_or(ExecutionError::FunctionNotFound)?;
        let func = match export {
            Extern::Func(f) => f,
            _ => return Err(ExecutionError::InvalidExport),
        };

        let ty = func.ty(&store);
        let params: Vec<ValType> = ty.params().collect();
        let results: Vec<ValType> = ty.results().collect();
        check_signature(&params, &results, &request.arguments)?;
        check_output_limit(&results, self.config.max_output_size)?;

        let args: Vec<Val> = request
            .arguments
            .iter()
            .copied()
            .map(WasmValue::to_val)
            .collect();
        let mut raw = vec![Val::I32(0); results.len()];
        func.call(&mut store, &args, &mut raw)
            .map_err(|e| classify_trap(&store, e))?;

        let mut return_values = Vec::with_capacity(raw.len());
        let mut out_size = 0usize;
        for val in &raw {
            let v = WasmValue::from_val(val)?;
            out_size = out_size
                .checked_add(v.encoded_size())
                .ok_or(ExecutionError::OutputTooLarge)?;
            if out_size > self.config.max_output_size {
                return Err(ExecutionError::OutputTooLarge);
            }
            return_values.push(v);
        }

        let remaining = store
            .get_fuel()
            .map_err(|_| ExecutionError::ExecutionTrap)?;
        let fuel_used = request
            .fuel_limit
            .checked_sub(remaining)
            .ok_or(ExecutionError::ExecutionTrap)?;

        Ok(ExecutionResult {
            return_values,
            fuel_used,
            module_id: module.id,
        })
    }
}

fn wasmtime_config(exec: &ExecutionConfig) -> Config {
    let mut config = Config::new();
    config.consume_fuel(true);
    config.wasm_features(WasmFeatures::FLOATS, false);
    config.wasm_features(WasmFeatures::SATURATING_FLOAT_TO_INT, false);
    config.wasm_simd(false);
    config.wasm_relaxed_simd(false);
    config.wasm_multi_value(true);
    config.wasm_bulk_memory(true);
    config.wasm_multi_memory(false);
    config.wasm_memory64(false);
    config.wasm_tail_call(false);
    config.wasm_extended_const(false);
    config.wasm_custom_page_sizes(false);
    config.wasm_wide_arithmetic(false);
    config.wasm_shared_everything_threads(false);
    config.wasm_branch_hinting(false);
    config.wasm_gc(false);
    let reserved = u64::from(exec.memory_limit_pages).saturating_mul(65536);
    config.memory_reservation(reserved);
    config.memory_may_move(false);
    config
}

fn check_signature(
    params: &[ValType],
    results: &[ValType],
    args: &[WasmValue],
) -> Result<(), ExecutionError> {
    if params.len() != args.len() {
        return Err(ExecutionError::InvalidFunctionSignature);
    }
    for (ty, arg) in params.iter().zip(args) {
        match (ty, arg) {
            (ValType::I32, WasmValue::I32(_)) | (ValType::I64, WasmValue::I64(_)) => {}
            (ValType::I32 | ValType::I64, _) => {
                return Err(ExecutionError::InvalidFunctionSignature)
            }
            _ => return Err(ExecutionError::InvalidFunctionSignature),
        }
    }
    for ty in results {
        if !matches!(ty, ValType::I32 | ValType::I64) {
            return Err(ExecutionError::InvalidFunctionSignature);
        }
    }
    Ok(())
}

fn check_output_limit(results: &[ValType], max: usize) -> Result<(), ExecutionError> {
    let mut size = 0usize;
    for ty in results {
        let n = match ty {
            ValType::I32 => 4,
            ValType::I64 => 8,
            _ => return Err(ExecutionError::InvalidFunctionSignature),
        };
        size = size.checked_add(n).ok_or(ExecutionError::OutputTooLarge)?;
        if size > max {
            return Err(ExecutionError::OutputTooLarge);
        }
    }
    Ok(())
}

fn classify_trap(store: &Store<GuestLimits>, err: wasmtime::Error) -> ExecutionError {
    if store.get_fuel().ok() == Some(0) {
        return ExecutionError::FuelExhausted;
    }
    let text = format!("{err:?}").to_ascii_lowercase();
    if text.contains("fuel") {
        ExecutionError::FuelExhausted
    } else if text.contains("memory") && (text.contains("limit") || text.contains("grow")) {
        ExecutionError::MemoryLimitExceeded
    } else {
        ExecutionError::ExecutionTrap
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::{wat_bytes, WasmLimits, WasmModule};

    fn module(src: &str) -> WasmModule {
        WasmModule::validate(&wat_bytes(src), &WasmLimits::default()).unwrap()
    }

    fn engine() -> ExecutionEngine {
        ExecutionEngine::new(ExecutionConfig::default()).unwrap()
    }

    fn run(src: &str, func: &str, args: Vec<WasmValue>) -> ExecutionResult {
        let eng = engine();
        let compiled = eng.compile(&module(src)).unwrap();
        eng.execute(
            &compiled,
            &ExecutionRequest {
                function: func.into(),
                arguments: args,
                fuel_limit: 100_000,
            },
        )
        .unwrap()
    }

    fn run_err(src: &str, func: &str, args: Vec<WasmValue>, fuel: u64) -> ExecutionError {
        let eng = engine();
        let compiled = eng.compile(&module(src)).unwrap();
        eng.execute(
            &compiled,
            &ExecutionRequest {
                function: func.into(),
                arguments: args,
                fuel_limit: fuel,
            },
        )
        .unwrap_err()
    }

    const ADD32: &str = r#"(module
        (func (export "add") (param i32 i32) (result i32)
          local.get 0 local.get 1 i32.add))"#;

    const ADD64: &str = r#"(module
        (func (export "add") (param i64 i64) (result i64)
          local.get 0 local.get 1 i64.add))"#;

    const PAIR: &str = r#"(module
        (func (export "pair") (param i64 i64) (result i64 i64)
          local.get 0 local.get 1))"#;

    const LOOP: &str = r#"(module
        (func (export "spin") (loop $l (br $l))))"#;

    const DIV: &str = r#"(module
        (func (export "div") (param i32 i32) (result i32)
          local.get 0 local.get 1 i32.div_s))"#;

    const GROW: &str = r#"(module
        (memory 1)
        (func (export "grow") (result i32)
          i32.const 32
          memory.grow))"#;

    const MEM1: &str = r#"(module (memory 1) (func (export "n")))"#;

    #[test]
    fn test_engine_initialization() {
        assert!(ExecutionEngine::new(ExecutionConfig::default()).is_ok());
        let bad = ExecutionConfig {
            fuel_limit: 0,
            ..ExecutionConfig::default()
        };
        assert!(matches!(
            ExecutionEngine::new(bad),
            Err(ExecutionError::InvalidConfiguration)
        ));
    }

    #[test]
    fn test_compile_valid_module() {
        let eng = engine();
        assert!(eng.compile(&module(ADD32)).is_ok());
    }

    #[test]
    fn test_execute_i32() {
        let r = run(ADD32, "add", vec![WasmValue::I32(2), WasmValue::I32(3)]);
        assert_eq!(r.return_values, vec![WasmValue::I32(5)]);
    }

    #[test]
    fn test_execute_i64() {
        let r = run(ADD64, "add", vec![WasmValue::I64(2), WasmValue::I64(3)]);
        assert_eq!(r.return_values, vec![WasmValue::I64(5)]);
    }

    #[test]
    fn test_multiple_arguments() {
        let r = run(ADD32, "add", vec![WasmValue::I32(10), WasmValue::I32(20)]);
        assert_eq!(r.return_values, [WasmValue::I32(30)]);
    }

    #[test]
    fn test_multiple_results() {
        let r = run(PAIR, "pair", vec![WasmValue::I64(1), WasmValue::I64(2)]);
        assert_eq!(r.return_values, [WasmValue::I64(1), WasmValue::I64(2)]);
    }

    #[test]
    fn test_function_not_found() {
        assert_eq!(
            run_err(ADD32, "nope", vec![], 1000),
            ExecutionError::FunctionNotFound
        );
    }

    #[test]
    fn test_invalid_function_signature() {
        assert_eq!(
            run_err(ADD32, "add", vec![WasmValue::I32(1)], 1000),
            ExecutionError::InvalidFunctionSignature
        );
    }

    #[test]
    fn test_invalid_argument_type() {
        assert_eq!(
            run_err(
                ADD32,
                "add",
                vec![WasmValue::I64(1), WasmValue::I64(2)],
                1000
            ),
            ExecutionError::InvalidFunctionSignature
        );
    }

    #[test]
    fn test_fuel_consumption() {
        let r = run(ADD32, "add", vec![WasmValue::I32(1), WasmValue::I32(1)]);
        assert!(r.fuel_used > 0);
        assert!(r.fuel_used < 100_000);
    }

    #[test]
    fn test_fuel_determinism() {
        let a = run(ADD32, "add", vec![WasmValue::I32(4), WasmValue::I32(5)]);
        let b = run(ADD32, "add", vec![WasmValue::I32(4), WasmValue::I32(5)]);
        assert_eq!(a.return_values, b.return_values);
        assert_eq!(a.fuel_used, b.fuel_used);
        assert_eq!(a.module_id, b.module_id);
    }

    #[test]
    fn test_fuel_exhaustion() {
        assert_eq!(
            run_err(LOOP, "spin", vec![], 50),
            ExecutionError::FuelExhausted
        );
    }

    #[test]
    fn test_infinite_loop_stops() {
        assert_eq!(
            run_err(LOOP, "spin", vec![], 200),
            ExecutionError::FuelExhausted
        );
    }

    #[test]
    fn test_memory_within_limit() {
        let eng = engine();
        assert!(eng.compile(&module(MEM1)).is_ok());
    }

    #[test]
    fn test_memory_limit_enforced() {
        let limits = WasmLimits {
            max_memory_pages: 1,
            ..WasmLimits::default()
        };
        let bytes = wat_bytes("(module (memory 8) (func (export \"n\")))");
        assert_eq!(
            WasmModule::validate(&bytes, &limits).unwrap_err(),
            ExecutionError::MemoryLimitExceeded
        );
    }

    #[test]
    fn test_memory_growth_rejected() {
        let cfg = ExecutionConfig {
            memory_limit_pages: 1,
            ..ExecutionConfig::default()
        };
        let eng = ExecutionEngine::new(cfg).unwrap();
        let compiled = eng.compile(&module(GROW)).unwrap();
        let err = eng
            .execute(
                &compiled,
                &ExecutionRequest {
                    function: "grow".into(),
                    arguments: vec![],
                    fuel_limit: 100_000,
                },
            )
            .unwrap_err();
        assert!(
            err == ExecutionError::MemoryLimitExceeded || err == ExecutionError::ExecutionTrap,
            "{err:?}"
        );
    }

    #[test]
    fn test_valid_output() {
        let r = run(ADD32, "add", vec![WasmValue::I32(1), WasmValue::I32(2)]);
        assert_eq!(r.return_values.len(), 1);
    }

    #[test]
    fn test_output_limit() {
        let cfg = ExecutionConfig {
            max_output_size: 8,
            ..ExecutionConfig::default()
        };
        let eng = ExecutionEngine::new(cfg).unwrap();
        let compiled = eng.compile(&module(ADD64)).unwrap();
        assert!(eng
            .execute(
                &compiled,
                &ExecutionRequest {
                    function: "add".into(),
                    arguments: vec![WasmValue::I64(1), WasmValue::I64(2)],
                    fuel_limit: 10_000,
                }
            )
            .is_ok());
    }

    #[test]
    fn test_output_too_large() {
        let cfg = ExecutionConfig {
            max_output_size: 3,
            ..ExecutionConfig::default()
        };
        let eng = ExecutionEngine::new(cfg).unwrap();
        let compiled = eng.compile(&module(ADD32)).unwrap();
        assert_eq!(
            eng.execute(
                &compiled,
                &ExecutionRequest {
                    function: "add".into(),
                    arguments: vec![WasmValue::I32(1), WasmValue::I32(2)],
                    fuel_limit: 10_000,
                }
            )
            .unwrap_err(),
            ExecutionError::OutputTooLarge
        );
    }

    #[test]
    fn test_runtime_trap() {
        assert_eq!(
            run_err(
                DIV,
                "div",
                vec![WasmValue::I32(1), WasmValue::I32(0)],
                10_000
            ),
            ExecutionError::ExecutionTrap
        );
    }

    #[test]
    fn test_division_trap() {
        assert_eq!(
            run_err(
                DIV,
                "div",
                vec![WasmValue::I32(i32::MIN), WasmValue::I32(-1)],
                10_000
            ),
            ExecutionError::ExecutionTrap
        );
    }

    #[test]
    fn test_trap_does_not_panic() {
        let _ = run_err(
            DIV,
            "div",
            vec![WasmValue::I32(1), WasmValue::I32(0)],
            10_000,
        );
    }

    #[test]
    fn test_same_module_same_args_same_result() {
        let a = run(ADD64, "add", vec![WasmValue::I64(7), WasmValue::I64(8)]);
        let b = run(ADD64, "add", vec![WasmValue::I64(7), WasmValue::I64(8)]);
        assert_eq!(a, b);
    }

    #[test]
    fn test_same_module_same_args_same_fuel_usage() {
        let a = run(ADD32, "add", vec![WasmValue::I32(9), WasmValue::I32(1)]);
        let b = run(ADD32, "add", vec![WasmValue::I32(9), WasmValue::I32(1)]);
        assert_eq!(a.fuel_used, b.fuel_used);
    }

    #[test]
    fn test_request_fuel_capped() {
        let eng = engine();
        let compiled = eng.compile(&module(ADD32)).unwrap();
        assert_eq!(
            eng.execute(
                &compiled,
                &ExecutionRequest {
                    function: "add".into(),
                    arguments: vec![WasmValue::I32(1), WasmValue::I32(1)],
                    fuel_limit: u64::MAX,
                }
            )
            .unwrap_err(),
            ExecutionError::InvalidConfiguration
        );
    }

    #[test]
    fn test_no_float_execution() {
        let bytes = wat_bytes("(module (func (export \"f\") (result f32) f32.const 1.0))");
        assert!(WasmModule::validate(&bytes, &WasmLimits::default()).is_err());
    }

    #[test]
    fn test_no_simd_execution() {
        let bytes =
            wat_bytes("(module (func (export \"s\") (result v128) v128.const i32x4 0 0 0 0))");
        assert!(WasmModule::validate(&bytes, &WasmLimits::default()).is_err());
    }

    #[test]
    fn test_no_threads_execution() {
        let bytes = wat_bytes("(module (memory 1 1 shared) (func (export \"n\")))");
        assert!(WasmModule::validate(&bytes, &WasmLimits::default()).is_err());
    }

    #[test]
    fn test_no_reference_types() {
        let bytes = wat_bytes("(module (func (export \"r\") (param externref)))");
        assert!(WasmModule::validate(&bytes, &WasmLimits::default()).is_err());
    }

    #[test]
    fn test_memory_bound() {
        assert_eq!(
            WasmModule::validate(
                &wat_bytes("(module (memory 64) (func (export \"n\")))"),
                &WasmLimits::default()
            )
            .unwrap_err(),
            ExecutionError::MemoryLimitExceeded
        );
    }

    #[test]
    fn test_fuel_bound() {
        assert_eq!(
            run_err(LOOP, "spin", vec![], 1),
            ExecutionError::FuelExhausted
        );
    }

    #[test]
    fn test_output_bound() {
        let cfg = ExecutionConfig {
            max_output_size: 0,
            ..ExecutionConfig::default()
        };
        let eng = ExecutionEngine::new(cfg).unwrap();
        let compiled = eng.compile(&module(ADD32)).unwrap();
        assert_eq!(
            eng.execute(
                &compiled,
                &ExecutionRequest {
                    function: "add".into(),
                    arguments: vec![WasmValue::I32(1), WasmValue::I32(2)],
                    fuel_limit: 1000,
                }
            )
            .unwrap_err(),
            ExecutionError::OutputTooLarge
        );
    }

    #[test]
    fn test_invalid_execution_state() {
        assert_eq!(
            run_err(ADD32, "add", vec![], 1000),
            ExecutionError::InvalidFunctionSignature
        );
    }
}
