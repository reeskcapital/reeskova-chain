# Build y tests de RSC Chain dentro de Docker (evita bloqueo de Windows en target/)
# Rust 1.88+ necesario: diesel_derives, home, time, etc. requieren rustc 1.86–1.88
FROM rust:1.88-bookworm

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Compilar y ejecutar solo tests del módulo chain/ (no consensus ni resto del crate)
RUN cargo test --release --lib chain::tests

# Opcional: imagen lista para ejecutar el binario
RUN cargo build --release
CMD ["./target/release/rsc_chain"]
