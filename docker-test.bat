@echo off
cd /d D:\rsc_chain
docker build -t rsc_chain .
docker run --rm rsc_chain cargo test --release --lib chain::tests
pause
