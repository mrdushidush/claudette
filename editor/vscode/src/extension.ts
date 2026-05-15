// Claudette VS Code extension.
//
// The extension is a thin wrapper around the `claudette` CLI: commands
// open an integrated terminal and run the right invocation. This is
// deliberate — Claudette has its own REPL and TUI, and re-implementing
// them inside a VS Code WebView would diverge from upstream. The value
// VS Code adds is editor-aware affordances ("ask about this selection")
// and keybinding-driven access to the CLI without leaving the editor.
//
// Future work: stream a structured JSON output mode from claudette into
// a chat panel WebView. That requires a Rust-side `--stdio-json` mode
// which doesn't exist yet.

import * as vscode from 'vscode';

/**
 * Settings keys live under the `claudette.*` namespace defined in
 * package.json. Reading them every command call honours the user's
 * latest setting without an extension reload.
 */
function getConfig() {
    const cfg = vscode.workspace.getConfiguration('claudette');
    return {
        binary: cfg.get<string>('binary', 'claudette'),
        terminalName: cfg.get<string>('terminalName', 'Claudette'),
        reuseTerminal: cfg.get<boolean>('reuseTerminal', true),
    };
}

/**
 * Return a terminal we can run Claudette in. When `reuseTerminal` is on
 * (default), we find or create one named after `terminalName` so the
 * panel doesn't fill up with one-shot terminals. With `reuseTerminal`
 * off, we always create a fresh terminal.
 */
function getTerminal(): vscode.Terminal {
    const cfg = getConfig();

    if (cfg.reuseTerminal) {
        const existing = vscode.window.terminals.find(t => t.name === cfg.terminalName);
        if (existing) {
            return existing;
        }
    }

    // Set the terminal's CWD to the active workspace folder if one is
    // open — Claudette's mission/forge/recall behavior is anchored on
    // the repo root, and most people would expect commands run via the
    // extension to act on the project that's open.
    const cwd = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;

    return vscode.window.createTerminal({
        name: cfg.terminalName,
        cwd,
    });
}

/**
 * Quote a single shell argument so it survives whatever shell the user
 * has configured. We can't rely on a specific shell (PowerShell, bash,
 * zsh, fish are all in the wild), so we use a conservative double-quoted
 * form: backslash-escape any backslashes and inner double quotes. This
 * matches PowerShell and POSIX double-quoted string parsing closely
 * enough for our purposes; the strings we send are user prompts and
 * code selections, not shell DSL.
 */
function shellQuote(s: string): string {
    return '"' + s.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\$/g, '\\$') + '"';
}

/**
 * Send a command line into the reused (or fresh) terminal. We bring the
 * terminal forward so the user sees output, not the silent edit panel.
 */
function runInTerminal(commandLine: string) {
    const term = getTerminal();
    term.show(true);
    term.sendText(commandLine, true);
}

/**
 * Build a single-line claudette invocation. We always quote the binary
 * so paths with spaces (e.g. `C:\Program Files\claudette\claudette.exe`)
 * work without surprise.
 */
function buildCmd(args: string[]): string {
    const { binary } = getConfig();
    const parts = [binary, ...args].map(p => p.includes(' ') || p.includes('"') ? shellQuote(p) : p);
    return parts.join(' ');
}

// ─── Commands ──────────────────────────────────────────────────────────

function cmdRepl() {
    runInTerminal(buildCmd([]));
}

function cmdTui() {
    runInTerminal(buildCmd(['--tui']));
}

function cmdDoctor() {
    runInTerminal(buildCmd(['--doctor']));
}

function cmdResume() {
    runInTerminal(buildCmd(['--resume']));
}

/**
 * Ask Claudette a one-shot question about the current selection. Pops a
 * quick-input for the user's question, then runs:
 *
 *   claudette "<question>\n\n<selection-with-file-path>"
 *
 * The selection is sent verbatim (no truncation) — Claudette handles
 * context budget on its end. We include the file path + line range so
 * the model can ground its answer in real source coordinates rather
 * than guessing.
 */
async function cmdAskSelection() {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
        vscode.window.showWarningMessage('Claudette: no active editor.');
        return;
    }

    const sel = editor.selection;
    if (sel.isEmpty) {
        vscode.window.showWarningMessage('Claudette: nothing is selected. Highlight some code first.');
        return;
    }

    const question = await vscode.window.showInputBox({
        title: 'Claudette: ask about this selection',
        prompt: 'What would you like Claudette to do with this selection?',
        placeHolder: 'e.g. "explain what this does", "refactor to use iterators", "find the bug"',
    });
    if (!question) {
        return; // user cancelled
    }

    const selectedText = editor.document.getText(sel);
    const filePath = vscode.workspace.asRelativePath(editor.document.uri, false);
    const startLine = sel.start.line + 1;
    const endLine = sel.end.line + 1;

    const prompt = [
        question,
        '',
        `Here is the selection (${filePath}:${startLine}-${endLine}):`,
        '',
        '```',
        selectedText,
        '```',
    ].join('\n');

    runInTerminal(buildCmd([prompt]));
}

/**
 * Launch forge mode on the current workspace with a prompt the user
 * supplies. Forge auto-bootstraps an ephemeral mission inside a git
 * repo, so this works as long as the workspace is git-initialized.
 */
async function cmdForge() {
    const ws = vscode.workspace.workspaceFolders?.[0];
    if (!ws) {
        vscode.window.showWarningMessage('Claudette: open a folder first — forge runs against the active workspace.');
        return;
    }

    const prompt = await vscode.window.showInputBox({
        title: 'Claudette: forge mode',
        prompt: 'Describe the change you want forge to make. It will Plan, Code, and Verify autonomously.',
        placeHolder: 'e.g. "add a --json flag to the export command that emits NDJSON"',
    });
    if (!prompt) {
        return;
    }

    runInTerminal(buildCmd(['--forge', prompt]));
}

// ─── Activation ────────────────────────────────────────────────────────

export function activate(ctx: vscode.ExtensionContext) {
    ctx.subscriptions.push(
        vscode.commands.registerCommand('claudette.repl', cmdRepl),
        vscode.commands.registerCommand('claudette.tui', cmdTui),
        vscode.commands.registerCommand('claudette.doctor', cmdDoctor),
        vscode.commands.registerCommand('claudette.resume', cmdResume),
        vscode.commands.registerCommand('claudette.askSelection', cmdAskSelection),
        vscode.commands.registerCommand('claudette.forge', cmdForge),
    );

    // Help the user discover commands on first install.
    const cfgKey = 'claudette.hasShownWelcome';
    if (!ctx.globalState.get<boolean>(cfgKey)) {
        ctx.globalState.update(cfgKey, true);
        vscode.window.showInformationMessage(
            'Claudette installed. Ctrl+Shift+P → "Claudette" to see commands. Selection + Ctrl+Alt+C asks about the selection.',
            'Open command palette',
        ).then(choice => {
            if (choice) {
                vscode.commands.executeCommand('workbench.action.showCommands');
            }
        });
    }
}

export function deactivate() {}
