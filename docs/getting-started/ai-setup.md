# AI Setup

Pushin uses a local OpenAI-compatible inference server. The model parses natural language into structure; Rust handles scheduling and date math.

## Built-in Setup

On first launch, the chat panel shows a setup card. Pick a model and click **Download**. Pushin downloads the model and a matching `llama-server`, then **starts it automatically** on `http://127.0.0.1:8080` — no extra click.

Recommended model tiers:

- **Lite — Qwen2.5 3B:** smallest and fastest
- **Recommended — Qwen2.5 7B:** best reliability for everyday planning
- **Powerful — Qwen2.5 14B:** strongest accuracy on capable machines

Pushin reads your machine's **RAM and GPU** and marks the best-fit model with a **"For your machine"** badge, so you don't have to guess.

Pushin also starts a small embedding model for vault recall on port `8181` after AI setup.

## Ready at launch

Once a model is downloaded, Pushin loads it into memory **while the opening screen is up** — the splash doubles as a loading screen and holds until the AI is ready. The app opens ready to plan; you won't see the calendar flash or a "start the AI" prompt before it's up. (First run, before any model exists, goes straight to the setup card above.)

## Ollama

If you already use Ollama:

```bash
ollama serve
ollama pull qwen2.5:3b
```

Then set the inference URL and model in Pushin settings.

## Reliability Note

Small local models are good at extraction and inconsistent at reasoning. Pushin keeps arithmetic, date resolution, and scheduling in deterministic Rust code.
