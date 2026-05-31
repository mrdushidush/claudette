# Claudette for VS Code

Wraps the `claudette` CLI in editor-aware commands. The heavy lifting still happens in Claudette itself — this extension exists to put the CLI's affordances behind keybindings and the command palette, plus one workflow that needs editor context: **"ask about this selection."**

## What's in here

| Command | Default keybinding | What it runs |
|---------|---------------------|--------------|
| `Claudette: Open REPL` | — | `claudette` in the integrated terminal |
| `Claudette: Open TUI` | — | `claudette --tui` |
| `Claudette: Doctor` | — | `claudette --doctor` |
| `Claudette: Resume Last Session` | — | `claudette --resume` |
| `Claudette: Ask About Selection` | `Ctrl+Alt+C` (`Cmd+Alt+C` on macOS) | `claudette "<your question>\n\n<selection>"` |
| `Claudette: Forge Mode` | — | `claudette --forge "<your prompt>"` |

All commands run inside an integrated terminal named `Claudette` (configurable). The terminal is reused across commands by default — turn that off in settings if you'd rather get a fresh terminal per invocation.

## Prerequisite

You need `claudette` on your `PATH`. If you don't, [install it](https://github.com/mrdushidush/claudette#install-in-30-seconds) first. The extension confirms it works by running `Claudette: Doctor`.

If your `claudette` binary is somewhere unusual, set the absolute path in settings:

```json
{
    "claudette.binary": "C:/Users/me/AppData/Local/Programs/claudette/claudette.exe"
}
```

## Building and installing locally

```bash
cd editor/vscode
npm install
npm run compile
npm run package           # produces claudette-X.Y.Z.vsix
code --install-extension claudette-0.1.0.vsix
```

That's it — no marketplace required.

## Why this is a thin wrapper

Claudette has a polished REPL and a fullscreen TUI of its own. Reimplementing either of those inside a VS Code WebView would duplicate work and drift from upstream. The piece an editor extension genuinely owns is selection-aware invocation — you've selected code, you have a question about it, you don't want to manually copy-paste it into another terminal. That's what `Claudette: Ask About Selection` is for, and it's where the extension earns its keep.

A future version may add a streaming chat WebView. That requires a structured JSON output mode in Claudette (tracked in the [issues](https://github.com/mrdushidush/claudette/issues)).

## Settings

| Setting | Default | Purpose |
|---------|---------|---------|
| `claudette.binary` | `claudette` | Path to the Claudette CLI. Leave as the name if it's on PATH. |
| `claudette.terminalName` | `Claudette` | Name of the integrated terminal used by all commands. |
| `claudette.reuseTerminal` | `true` | Reuse the same terminal across commands. Off → fresh terminal each time. |

## License

MIT OR Apache-2.0, same as the rest of the Claudette repo.
