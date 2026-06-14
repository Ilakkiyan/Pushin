# AI Setup

Pushin uses a local OpenAI-compatible inference server. The model parses natural language into structure; Rust handles scheduling and date math.

## Built-in Setup

On first launch, the chat panel shows a setup card. Pick a model and click **Download**. Pushin downloads the model and a matching `llama-server`, then starts it on `http://127.0.0.1:8080`.

Recommended model tiers:

- **Lite — Qwen2.5 3B:** smallest and fastest
- **Recommended — Qwen2.5 7B:** best reliability for everyday planning
- **Powerful — Qwen2.5 14B:** strongest accuracy on capable machines

Pushin also starts a small embedding model for vault recall on port `8181` after AI setup.

## Ollama

If you already use Ollama:

```bash
ollama serve
ollama pull qwen2.5:3b
```

Then set the inference URL and model in Pushin settings.

## Reliability Note

Small local models are good at extraction and inconsistent at reasoning. Pushin keeps arithmetic, date resolution, and scheduling in deterministic Rust code.
