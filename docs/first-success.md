# First success — pick a path, get a real win

Four copy-paste recipes, each ending in something you can verify worked. All of them assume you've done the two-minute install from the [README](../README.md#get-started-in-2-minutes): installer one-liner → `ollama pull qwen3.5:4b` → `claudette --doctor` shows green.

Want a broader "what can I even ask it?" catalog instead of a guided path? → [show-me.md](show-me.md).

| Path | You get | Time |
|------|---------|------|
| [Safe first steps](#safe) | Notes, todos, weather — fully local | 3 min |
| [Local coding agent](#coding) | A build-and-test-verified commit on a branch, made by Forge | 10 min |
| [Maximum privacy](#airgap) | A complete coding session under an enforced air-gap | 10 min |
| [Private assistant + Telegram](#assistant) | Voice assistant on your phone + morning briefing | 20 min |

---

<a id="safe"></a>
## 🌱 Safe first steps (fully local, nothing can leave)

Notes and todos live in plain files under `~/.claudette/`; weather uses a keyless public API. Start `claudette` and paste these one at a time:

```text
take a note: the storage closet key lives behind the printer
add a todo: review the quarterly report by Friday
what's the weather in Tel Aviv tomorrow?
list my open todos
what did I write down about the storage closet?
```

**You should see:** each note/todo confirmed as saved, the weather with real numbers, and the last prompt finding your note *by meaning* — even if you phrase it differently than you wrote it.

**If not:** a hang of 1–3 minutes on the very first prompt is the model loading into memory, not a bug — see [troubleshooting.md](troubleshooting.md#the-first-prompt-hangs-for-13-minutes-then-answers).

---

<a id="coding"></a>
## 🛠️ Local coding agent (Forge, on any repo of yours)

Forge runs an autonomous **Planner → Coder → Verifier** pipeline. The Verifier actually builds and runs your project's tests each round — a diff that doesn't compile or breaks a test can't pass. No GitHub token needed for this recipe: on a local repo, Forge commits to an isolated branch and never touches your working branch.

```sh
cd ~/code/any-git-repo-you-own      # any git repo under $HOME
claudette --forge "add a --version flag that prints the package version"
```

(Swap the task for anything small and testable in your project.)

**You should see** the phases stream past:

```text
forge: planner                 # localizes the code, writes a short plan
forge: coder (round 0)         # makes the edit, commits to the mission branch
forge: build + test            # cargo check / pytest / go test / npm test
forge: verifier   score=9 pass=true
forge: changes committed locally at <your repo> (ephemeral/local mission — no
       GitHub PR target). Review with `git log`/`git diff`, then push + open a
       PR manually if you want one.
```

Then inspect and take the win (the branch is named `claudette-mission/<slug>-<timestamp>`):

```sh
git branch --list 'claudette-mission/*'   # find the mission branch
git log --oneline <that-branch>           # the verified commit
git diff main...<that-branch>             # the full change
git merge <that-branch>                   # keep it (or git branch -D to toss it)
```

**Success =** the branch exists, the diff does what you asked, and the build/test gate ran before the commit was made. Your working branch was never touched.

**Next:** point it at a GitHub repo with `/brownfield owner/repo` + `/forge <task>` — that path ends in a real PR behind a plan-and-diff `[y/N]` review gate. Full pipeline: [forge.md](forge.md).

---

<a id="airgap"></a>
## 🔒 Maximum privacy: the enforced air-gap

This is Claudette's wedge: `--offline` is not a promise, it's an enforced mode with two guard layers (in-process HTTP **and** subprocesses) and an integration test that drives every networked tool to prove it refuses. Nothing else in [comparison.md](comparison.md)'s matrix has an enforced, tested equivalent.

**1. See exactly what's allowed:**

```sh
claudette --offline --doctor
```

**You should see:** the doctor report plus the offline allow-list — your local model server and loopback, nothing else.

**2. Try to break it.** Start `claudette --offline` and ask it to reach out:

```text
search the web for rust news
push this repo to github
run: bash -c "curl example.com"
```

**You should see:** every one refuse with a clear `blocked by offline mode` error. The `bash` tool refuses **wholesale** under `--offline` — a raw shell is an escape hatch no allow-list can inspect, so it isn't filtered, it's off.

**3. Now do real work in the same session:**

```text
map this repo and explain the module layout
find every TODO in src/ and list them by file
add a unit test for the parse_config function, then run the tests
```

**Success =** file ops, search, repo-map, edits, and test runs all work normally — the brain is local, so the air-gap costs you nothing for coding. Every place a byte *could* leave the machine in normal mode is inventoried in [PRIVACY.md](../PRIVACY.md).

---

<a id="assistant"></a>
## 🏠 Private assistant + Telegram (full flavor)

Three commands from lean install to a voice assistant on your phone. This path uses the **full** flavor (the cloud integrations are deliberately not in the default build).

**1. Get the full binary** (skip if you installed with `--features integrations`):

```sh
CLAUDETTE_FLAVOR=full curl -fsSL https://raw.githubusercontent.com/mrdushidush/claudette/main/install.sh | sh   # Linux / macOS
$env:CLAUDETTE_FLAVOR='full'; iwr -useb https://raw.githubusercontent.com/mrdushidush/claudette/main/install.ps1 | iex  # Windows
```

**2. Connect Google** (one-time OAuth; full walkthrough with screenshots: [google_setup.md](google_setup.md)):

```sh
claudette --auth-google calendar
claudette --auth-google gmail
```

**You should see:** a browser window for consent, then `claudette "what's on my calendar tomorrow?"` answers with your real events. Your inbox is read-only — Claudette can search and read mail, never send.

**3. Put it on your phone.** Get a bot token from `@BotFather` in Telegram, then:

```sh
export TELEGRAM_BOT_TOKEN=123456:ABC...   # $env:TELEGRAM_BOT_TOKEN= on Windows
claudette --telegram --chat any           # lock to --chat <your-id> once it works
```

**You should see:** your bot answer a text message from your phone. Send a voice note and it transcribes (Whisper, local — pull a model to `~/.claudette/models/ggml-large-v3-turbo.bin` first) and answers; type `/voice` for spoken replies. Telegram default-denies anything destructive — no shell, no git push from your phone.

**Bonus — the morning briefing:**

```sh
claudette --briefing        # 07:00 weekdays: calendar + weather + email digest
```

**Success =** you ask your phone "what's on my calendar tomorrow?" from the supermarket and get your real schedule back, off your own hardware.
