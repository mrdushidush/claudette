# Hero demo for the README: a REPL fix of a failing pytest, with the [y/N] gate.
# Run from D:\demo:  vhs hero.tape
# Reset the toy repo between takes:  git -C D:\demo\slugs checkout -- .
#
# One gate only: the REPL runs in workspace-write mode, so run_tests passes
# without a prompt and the edit is the only [y/N]. Every Wait is +Line (the
# cursor's line), never +Screen, so a prompt that already scrolled by can't
# satisfy a later wait. The spinner is off: with it on, every frame differs
# while the model thinks and the encoder can't trim that stretch as idle.

Output out/frames/

Set Shell "powershell"
Set FontSize 16
Set Width 1150
Set Height 680
Set Padding 16
Set TypingSpeed 35ms
Set CursorBlink false

Hide
Type "$env:PATH = 'D:\demo\bin;' + $env:PATH; $env:OLLAMA_HOST = 'http://localhost:1234'; $env:CLAUDETTE_OPENAI_COMPAT = '1'; $env:CLAUDETTE_MODEL = 'qwen3.6-35b-a3b-mtp@iq3_s'; $env:CLAUDETTE_RECALL_DISABLE = '1'; $env:CLAUDETTE_NO_SPINNER = '1'; Set-Location D:\demo\slugs; function prompt { '> ' }; Clear-Host"
Enter
Sleep 2s
Show

Type "python -m pytest -q"
Enter
Sleep 3s

Type "claudette"
Enter
Wait+Line@60s /›\s*$/
Sleep 1s

Type "test_punctuation fails. Fix slugify so the tests pass; do not edit the tests. Check with run_tests."
Enter

# The edit, shown as a colored diff, waits for a single key. No Enter after
# the y: the gate reads one keypress, and an Enter would answer the next one.
Wait+Line@180s /Allow\?/
Sleep 2.5s
Type "y"
Sleep 1s

Wait+Line@240s /›\s*$/
Sleep 2s

Type "/exit"
Enter
Sleep 2s

Type "python -m pytest -q"
Enter
Sleep 4s
