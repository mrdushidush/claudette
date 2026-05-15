# Deploy Claudette on a server

The fastest way to put Claudette on a small VPS, a Raspberry Pi, or a home server: use the bundled `docker-compose.yml`. It brings up Ollama + Claudette together, runs Claudette in Telegram-bot mode, and keeps everything on local disk volumes.

This is the "Telegram bot on a tower at home" use case — talk to Claudette from your phone, all data stays in your house.

---

## What you'll need

- A Linux host with Docker + Compose installed. Raspberry Pi 5 (4–8 GB), Hetzner CX21, DigitalOcean droplet, or an old desktop all work fine.
- A Telegram bot token from [@BotFather](https://t.me/BotFather). Free, 60 seconds.
- ~3 GB free disk (Ollama image + Claudette image + one small brain).
- ~2 GB RAM headroom (the smallest brain is ~1.2 GB resident; the rest is overhead).

**You do not need a GPU.** The compose file is tuned for CPU inference with a small Qwen brain. See [`hardware.md#no-gpu-cpu-only-mode`](hardware.md#no-gpu-cpu-only-mode) for throughput expectations.

---

## Five-minute deploy

```bash
# 1. Clone (or copy Dockerfile + docker-compose.yml to your server)
git clone https://github.com/mrdushidush/claudette
cd claudette

# 2. Configure
cat > .env <<EOF
TELEGRAM_BOT_TOKEN=123456:your-bot-token-here
# Optional:
# BRAVE_API_KEY=...
# GITHUB_TOKEN=ghp_...
EOF

# 3. Build + start
docker compose up -d --build

# 4. Pull a brain into Ollama (one-time per machine)
docker compose exec ollama ollama pull qwen2.5:1.5b

# 5. Watch the bot connect
docker compose logs -f claudette
```

You should see Claudette announce it's polling Telegram. Open Telegram on your phone, find your bot, send a message. That's it.

---

## Picking the right brain for your hardware

| Hardware | Pull | RAM footprint | Notes |
|----------|------|---------------|-------|
| Raspberry Pi 4 (4 GB) | `qwen2.5:0.5b` | ~700 MB | Slow but functional |
| Raspberry Pi 5 (8 GB) | `qwen2.5:1.5b` | ~1.4 GB | The sweet spot for a Pi bot |
| $5 VPS (1 GB) | `qwen2.5:0.5b` | ~700 MB | Tight but works |
| $10 VPS (2 GB) | `qwen2.5:1.5b` | ~1.4 GB | Comfortable |
| $20 VPS / old desktop (4 GB+) | `qwen2.5:3b` | ~2.5 GB | Visibly smarter |
| Anything with a GPU | `qwen3.5:4b` | ~3.4 GB VRAM | Switch to GPU-passthrough — see below |

Pin the brain in your compose `environment:` block:

```yaml
environment:
  CLAUDETTE_MODEL: qwen2.5:1.5b
```

Then `docker compose up -d` to apply.

---

## GPU passthrough (NVIDIA)

If you have a CUDA-capable GPU on the host, give Ollama access to it:

```yaml
services:
  ollama:
    image: ollama/ollama:latest
    deploy:
      resources:
        reservations:
          devices:
            - driver: nvidia
              count: all
              capabilities: [gpu]
```

You'll need [`nvidia-container-toolkit`](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/install-guide.html) on the host. With it, the same `qwen3.5:4b` you'd run locally fits and is dramatically faster.

ROCm (AMD) and Metal (macOS) work too — Ollama documents the relevant flags. Outside the scope of this doc.

---

## Backing up your data

Two named volumes hold everything:

```bash
docker run --rm -v claudette-data:/data -v "$PWD":/backup alpine \
    tar -czf /backup/claudette-data.tar.gz -C /data .
```

`claudette-data` holds your sessions, notes, recall index, tokens, missions. `ollama-models` holds the downloaded weights and can be re-downloaded any time, so you don't *need* to back it up unless your upstream connection is slow.

---

## Updating

```bash
git pull
docker compose up -d --build
```

Compose rebuilds the Claudette image and restarts the service. The data volumes survive the restart.

---

## Without docker-compose (single container)

If you'd rather manage Ollama on the host directly:

```bash
# 1. Ollama on the host
curl -fsSL https://ollama.com/install.sh | sh
ollama pull qwen2.5:1.5b
ollama serve &  # or use the systemd unit Ollama installs

# 2. Claudette as a container, hitting host Ollama
docker build -t claudette .
docker run -d --name claudette --restart unless-stopped \
    -e TELEGRAM_BOT_TOKEN="123:abc" \
    -e OLLAMA_HOST="http://host.docker.internal:11434" \
    --add-host=host.docker.internal:host-gateway \
    -v claudette-data:/home/claudette/.claudette \
    claudette --telegram
```

---

## systemd alternative (no Docker)

If Docker is overkill for your box (e.g. running directly on a Pi with the system Ollama), install Claudette natively and wrap it in a systemd unit:

```ini
# /etc/systemd/system/claudette.service
[Unit]
Description=Claudette Telegram bot
After=network-online.target ollama.service
Wants=network-online.target

[Service]
Type=simple
User=pi
Environment="TELEGRAM_BOT_TOKEN=123456:your-token"
Environment="CLAUDETTE_MODEL=qwen2.5:1.5b"
ExecStart=/home/pi/.local/bin/claudette --telegram
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now claudette
journalctl -u claudette -f
```

Install Claudette with the one-line installer first: `curl -fsSL https://raw.githubusercontent.com/mrdushidush/claudette/main/install.sh | sh`.

---

## Privacy note

By default Claudette warns at startup when `OLLAMA_HOST` points anywhere except loopback — a safety reminder that "your prompts are about to leave the local machine." Inside docker-compose, the bridge-network hostname `ollama` is not loopback even though it's effectively local, so the bundled `docker-compose.yml` sets `CLAUDETTE_ALLOW_REMOTE_OLLAMA=1` to acknowledge the configuration and silence the warning. **Only set this when you actually understand the topology** — it's the same env var that would silence the warning if Ollama were on a different host across the open internet.

For the full inventory of what leaves the machine and when, see [`../PRIVACY.md`](../PRIVACY.md).
