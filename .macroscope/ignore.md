# Repository-wide Macroscope ignore (code review + any check-run agents).
#
# A custom ignore file REPLACES Macroscope's built-in defaults, so this copies
# their default "base" patterns verbatim to preserve them, then adds this repo's
# own recorded files on top. https://docs.macroscope.com/
#
# Macroscope's default *test-file* patterns are deliberately NOT copied here: we
# want Macroscope to review test code. Recorded cassettes and binary fixtures
# under tests/ stay ignored via the base binary/data patterns plus the explicit
# cassettes rule below, so only real test *code* is reviewed.

# ---- Macroscope default base patterns (copied to preserve them) ----
**/.git/**
**/__pycache__/**
**/.pytest_cache/**
**/.mypy_cache/**
**/.ruff_cache/**
**/venv/**
**/.venv/**
**/node_modules/**
**/site-packages/**
**/.pnpm-store/**
**/__Snapshots__/**
**/__snapshots__/**
**/.agents/skills/**
**/.claude/skills/**
**/.github/skills/**
**/bower_components/**
**/jspm_packages/**
**/.next/**
**/.svelte-kit/**
**/vendor/**
**/_vendor/**
**/third_party/**
**/Pods/**
**/.bundle/**
build/**
env/**
ENV/**
**/target/**
**/generated/**
**/intermediates/**
**/generated_sources/**
**/generated-sources/**
**/generated-src/**
**/src/main/generated/**
**/*.min.js
**/*.min.css
**/*_pb.d.ts
**/*_pb.js
**/*.pb.go
**/*_pb2.py
**/*_pb2_grpc.py
**/*_pb2.pyi
**/*.grpc.swift
**/*.pb.swift
**/*.sql.go
**/*.designer.cs
**/*.g.dart
**/*.pb.dart
**/*_pb.rb
**/go.mod
**/package.json
**/*.pbxproj
**/*.xcstrings
**/*.strings
**/*.properties
**/pom.xml
**/Package.swift
**/bun.lock
**/.eslintrc
**/.eslintignore
**/go.sum
**/package-lock.json
**/pnpm-lock.yaml
**/yarn.lock
**/Package.resolved
**/*.jpg
**/*.jpeg
**/*.png
**/*.gif
**/*.svg
**/*.ico
**/*.webp
**/*.bmp
**/*.tiff
**/*.woff
**/*.woff2
**/*.ttf
**/*.eot
**/*.otf
**/*.mp3
**/*.mp4
**/*.wav
**/*.avi
**/*.mov
**/*.mkv
**/*.flac
**/*.ogg
**/*.srt
**/*.zip
**/*.tar
**/*.gz
**/*.rar
**/*.7z
**/*.bz2
**/*.pdf
**/*.doc
**/*.docx
**/*.xls
**/*.xlsx
**/*.ppt
**/*.pptx
**/*.db
**/*.sqlite
**/*.sqlite3
**/*.parquet
**/*.avro
**/*.arrow
**/*.npy
**/*.pkl
**/*.jsonl
**/*.onnx
**/*.tflite
**/*.h5
**/*.safetensors
**/*.exe
**/*.dll
**/*.so
**/*.dylib
**/*.bin
**/*.pyc
**/*.class
**/*.o
**/*.a
**/*.wasm
**/*.cer
**/*.pem
**/*.p12
**/*.stringsdict
**/*.snap
**/*.adoc
**/*.arb
**/*.lock
**/*.po
**/*.fbx
**/*.log
**/*.xib
**/*.meta
**/*.kml
**/*.prefab
**/*.eml
**/*.csv
**/*.grpc.reflection
**/*.js.map

# ---- This repo (monty): recorded / generated / scratch files ----

# Vendored typeshed (copied upstream, incl. abc.pyi), not authored here. Only
# this directory is ignored -- authored stubs like the public
# crates/monty-python/python/pydantic_monty/_monty.pyi stay reviewed.
crates/monty-typeshed/**

# Scratch divergence probes written by the review-usability skill; throwaway,
# not shipped code.
playground/**

# Fuzz corpora and datatest fixtures are data, not code.
crates/fuzz/corpus/**
crates/monty-datatest/**/*.txt
crates/monty-datatest/**/*.json

# `./limitations/` is intentionally NOT ignored: the correctness instructions
# check behaviour changes for parity with a documented divergence entry.
