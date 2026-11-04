# Stopgaps

| stopgap | why | removed when |
|---|---|---|
| gh call log | ~/.local/bin/gh wrapper logs every gh call to /Volumes/Data/tmp/roger/gh-calls.log | 2026-11-04 | remove once the GitHub API consumers are known and capped |
| SelyraAI git over HTTPS | repo-local `url.https://github.com/.insteadOf git@github.com:` and `credential.https://github.com.helper` = gh auth git-credential in /Volumes/Data/myai/SelyraAI/.git/config: GitHub SSH refused this machine on and off (2026-11-04) | 2026-11-05 04:47: test SSH again and ask the user whether to switch back (remove the two settings) |
