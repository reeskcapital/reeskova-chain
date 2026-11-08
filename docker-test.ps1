# Ejecutar tests de chain dentro de Docker (todo desde D:\rsc_chain)
Set-Location D:\rsc_chain
docker build -t rsc_chain .
docker run --rm rsc_chain cargo test --release --lib chain::tests
