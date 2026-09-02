# Motor de inferencia vendorizado

Copia literal de `inference-engine/crates/` del repo
`github.com/charlessonamericantrading/skynet` (el motor de inferencia propio de
Skynet: parser GGUF, kernels cuantizados AVX2/FMA con camino escalar, KV-cache,
paged attention, prefix cache, forward de Llama / Gemma 4 / Qwen2.5 / Qwen3 /
Phi-3 / PhiMoE). Cero dependencias externas. `vulkan-backend` queda fuera a
propósito (necesita `glslc`; la GPU se descartó en el proyecto de origen).

- Commit de origen: `eea869c533049d85bc36e8aca257dc5246f7aa41` (2026-09-03)
- Crates: gguf, tensor-core, model-core, llama, gemma4, qwen2, qwen3, phi3, phimoe, server
- Resincronizar: `bash scripts/sync-inference-engine.sh [ruta-al-repo-skynet]`

Regla: NO se edita nada aquí. Un cambio que el motor necesite se hace en el repo
de origen y se vuelve a sincronizar; así el motor sigue teniendo una sola fuente
de verdad (GRAMMAR.md §3.233).
