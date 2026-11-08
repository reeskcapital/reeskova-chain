//! Motor de ejecución WebAssembly de RSC Chain v1.
//!
//! `rsc-execution` valida y ejecuta **core modules** bajo el entorno
//! [`EXECUTION_ENVIRONMENT_ID`]. No posee estado, no firma, no habla con
//! consenso ni con storage. El ABI de contratos y el gas económico son
//! capas posteriores.
//!
//! Identidad de módulo = SHA-256 de los **bytes exactos** aceptados
//! (`rsc-crypto`). El fuel es presupuesto determinista, no tiempo de reloj.

#![forbid(unsafe_code)]

pub mod runtime;
pub mod wasm;

pub use runtime::{
    CompiledModule, ExecutionConfig, ExecutionEngine, ExecutionRequest, ExecutionResult, WasmValue,
};
pub use wasm::{WasmLimits, WasmModule, WasmModuleId};

/// Identificador estable del execution environment. Un bump de Wasmtime
/// o del perfil de features es un entorno nuevo, no un `cargo update`.
pub const EXECUTION_ENVIRONMENT_ID: &str = "RSC-WASM-V1";

/// Runtime fijado para [`EXECUTION_ENVIRONMENT_ID`].
pub const WASMTIME_VERSION: &str = "48.0.2";

/// Costes de operadores: tabla por defecto de Wasmtime 48.0.2.
/// Forma parte de RSC-WASM-V1. No se reescribe por plataforma.
pub const FUEL_COST_TABLE: &str = "wasmtime-48.0.2-default";

/// Error público sanitizado. Nunca incluye paths, secretos ni el `Display`
/// crudo de Wasmtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ExecutionError {
    #[error("invalid module")]
    InvalidModule,
    #[error("validation failed")]
    ValidationFailed,
    #[error("compilation failed")]
    CompilationFailed,
    #[error("invalid export")]
    InvalidExport,
    #[error("function not found")]
    FunctionNotFound,
    #[error("invalid function signature")]
    InvalidFunctionSignature,
    #[error("invalid argument")]
    InvalidArgument,
    #[error("fuel exhausted")]
    FuelExhausted,
    #[error("memory limit exceeded")]
    MemoryLimitExceeded,
    #[error("execution trap")]
    ExecutionTrap,
    #[error("output too large")]
    OutputTooLarge,
    #[error("unsupported feature")]
    UnsupportedFeature,
    #[error("unsupported import")]
    UnsupportedImport,
    #[error("component not supported")]
    ComponentNotSupported,
    #[error("start function not supported")]
    StartFunctionNotSupported,
    #[error("invalid configuration")]
    InvalidConfiguration,
    #[error("engine initialization failed")]
    EngineInitializationFailed,
    #[error("module too large")]
    ModuleTooLarge,
    #[error("too many memories")]
    TooManyMemories,
    #[error("too many tables")]
    TooManyTables,
    #[error("too many functions")]
    TooManyFunctions,
    #[error("too many globals")]
    TooManyGlobals,
    #[error("too many imports")]
    TooManyImports,
    #[error("too many exports")]
    TooManyExports,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_id_is_stable() {
        assert_eq!(EXECUTION_ENVIRONMENT_ID, "RSC-WASM-V1");
        assert_eq!(WASMTIME_VERSION, "48.0.2");
    }
}
