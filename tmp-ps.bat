@echo off
tasklist /FI "IMAGENAME eq cargo.exe"
tasklist /FI "IMAGENAME eq cl.exe"
tasklist /FI "IMAGENAME eq rustc.exe"
tasklist /FI "IMAGENAME eq link.exe"
