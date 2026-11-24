# Nothing is lost

DOC.md promises that Brouyon never writes over a change it hasn't shown you, and that nothing is lost. Where it breaks, a note is gone with no error and no copy in the Trash.

A test runs one order of events. `NothingLost.tla` describes `flush`, `refresh_open_note`, the trash and `Store` one system call at a time, beside another program that can act between any two of them. TLC tries every order.

## Run it

```sh
verify/check
```

It takes about a minute on 16 cores and needs `mise`, which fetches the Java it pins. The first run downloads TLC 1.7.4 into `target/verify/` and checks its hash. It reads no notes and opens no window. CI runs it.

| Line | What it says |
|---|---|
| `ok promise` | With the finding set aside, every rule holds in every state within the bounds. |
| `KNOWN finding-…` | The finding below is still in the code, and breaks only the rules recorded for it. |
| `ok mistake-…` | A mistake was caught. Where a commit is named, the mistake was in the code before it. |
| `WRONG …` | A rule broke, a finding went away, or a mistake got through. |
| `DRIFT …` | The covered code changed since the model was confirmed to describe it. |

It exits with 0 when all is as recorded, 1 for `WRONG`, and 2 for `DRIFT` alone. `verify/check --trace CASE RULE` tells a counterexample in the code's words.

## When the code changes

`verify/check --drift` names each covered function that changed, and each new one that handles notes' text. For each of them:

1. Read its diff against the action the table below gives for it.
2. Make the model do what the code now does. A step is one system call. A call that writes between a check and its act belongs in `Gap`.
3. Add a `Broken` mistake, a case in `check`, and a rule for any new way to lose text.
4. Run `verify/check`. A rule that breaks is a bug in the code until a replay shows otherwise: see below. Don't weaken the rule.
5. Run `verify/check --confirm` and commit the table with the code. It takes the code as it is, so it proves nothing without the steps above.

## Bug, or wrong model?

Replay the steps on the real code, in a test in `src/app.rs`, one line for each action. Until the app does it, the model may be wrong.

| Step | In a test |
|---|---|
| `Type` | `cx.simulate_input("…")` |
| `Compose`, `Unmark` | `compose`, `keep_composed` |
| `SaveTimer` | `wait(cx, SAVE_DELAY)` |
| `LoseFocus` | `cx.deactivate_window()` |
| `Select`, `ShowNotes` | `keys(cx, "ctrl-n")`, `keys(cx, "alt-down")` |
| `IdleMove` | `wait(cx, IDLE)` |
| `ShowTrash`, `MoveToTrash` | `keys(cx, "ctrl-shift-t")`, `keys(cx, "ctrl-shift-backspace")` |
| `Ask`, `Confirm` | `click("delete-forever", cx)`, `click("confirm", cx)` |
| `Quit` | `cx.simulate_close()` |
| `FlushFails` | `fs::remove_dir_all(dir.path())` before the save |
| `TheyWrite`, `TheyDelete`, and in the trash | `fs::write`, `fs::remove_file` |
| `Sync` | `notice_changes(&view, cx)` |

`replays.patch` holds the replays of the finding. `race.c` plays the other program between two system calls, on Linux:

```sh
git apply verify/replays.patch
mkdir -p target/verify && gcc -shared -fPIC -o target/verify/race.so verify/race.c -ldl
LD_PRELOAD="$PWD/target/verify/race.so" cargo test replay_ -- --test-threads=1
git apply -R verify/replays.patch
```

## Findings

**A check and its act are two system calls** (`finding-gap`; SaveKeepsTheirs, RemoveKeepsTheirs, PurgeKeepsTheirs). It is open, and was replayed on the real code. Another program that writes between the two is overwritten or deleted. It takes the same shape in `Store::save`, `remove_unchanged` and `purge`.
1. `Store::save` reads the file. It holds what was loaded.
2. Another program replaces the file.
3. `Store::save` renames its temporary file over it, and returns `Written`.

The moment is microseconds wide. `create`, `trash` and `restore` don't have it: `move_new` looks and moves in one system call.

## What it covers

Bounds: three file names and three texts for `promise`, two names and two or three texts for the rest. The number of steps isn't bounded. Nor is how often the other program acts or a system call fails.

It assumes that:
- Brouyon is the only Brouyon on the folder on this computer. Two would share the temporary file that `temp_name` names.
- The other program replaces whole files at once, as Dropbox does. A file written in place, and read half-written, is left out.
- Handlers run one at a time on the main thread.
- The file system can refuse to replace a file. Where it can't, such as on NFS, `move_new_in_two` looks and moves in two calls.

It leaves out: several notes folders (`set_notes_dir` counts as leaving a note), Empty Trash, which asks for every note to go, seen or not, files that aren't UTF-8, text that is only white space, dates, the list, power loss, a kill before the save timer, and what Dropbox does with two versions. After a save that failed, and said so, quitting may drop the text: `may_leave` means it to.

A pass is about this model within these bounds, not the whole app. The table is what ties it to the code.

## Covered code

| File | Function | In the model | Confirmed |
|---|---|---|---|
| `src/store.rs` | `save` | SaveRead, SaveRename | `bd105301ad82` |
| `src/store.rs` | `remove_unchanged` | RemoveRead, RemoveUnlink | `fa757478bd8b` |
| `src/store.rs` | `create` | CreateMove | `7b75029f0095` |
| `src/store.rs` | `trash` | TrashMove | `34c47a7e06a4` |
| `src/store.rs` | `restore` | Restore | `997d7de4b075` |
| `src/store.rs` | `purge` | Confirm, PurgeUnlink | `ec6db3b637aa` |
| `src/store.rs` | `remove_if_holds` | RemoveRead, RemoveUnlink, Confirm, PurgeUnlink | `1dbdd4a8fefc` |
| `src/store.rs` | `gone` | RemoveRead, RemoveUnlink, PurgeUnlink | `f2b029774ab1` |
| `src/store.rs` | `write_temp` | SaveRead, CreateMove | `43e4dce64dd1` |
| `src/store.rs` | `replace` | SaveRename | `ccd4b6f3c93b` |
| `src/store.rs` | `move_to_free_name` | FreeNames | `76ad09b436f2` |
| `src/store.rs` | `move_new` | CreateMove, TrashMove, Restore | `d050286bc60f` |
| `src/store.rs` | `move_new_in_two` | Left out: the assumption about the file system | `097c8f1da99a` |
| `src/store.rs` | `empty_trash` | Left out: it asks for every note to go, seen or not | `d99e3e7b6230` |
| `src/store.rs` | `write_if` | The order of SaveRead and SaveRename, and FlushFails | `2eb83e1dfa8c` |
| `src/store.rs` | `write_atomically` | Left out: it writes the settings | `3830974b9ee8` |
| `src/store.rs` | `temp_name` | The assumption that nothing else uses the temporary file | `2a307189254f` |
| `src/app.rs` | `follow_editor` | Type, Compose: which changes are looked at | `df636c56308d` |
| `src/app.rs` | `on_editor_notify` | Type, Compose, SaveTimer | `16f29149f73c` |
| `src/kept.rs` | `changed` | Type, Compose | `e5bc3f163e30` |
| `src/kept.rs` | `saved` | Saved, SavedNothing | `05faa017071c` |
| `src/kept.rs` | `refused` | FlushFails | `3b549d5d5ec4` |
| `src/kept.rs` | `kept` | Flush, RefreshOpenNote | `69b80948ca32` |
| `src/kept.rs` | `warned` | Quit | `85e2230cc164` |
| `src/kept.rs` | `may_leave` | Quit | `2d2cda7f9e61` |
| `src/kept.rs` | `may_leave_anyway` | Quit | `02178f2091af` |
| `src/kept.rs` | `quit` | Quit | `d932277ee750` |
| `src/kept.rs` | `close` | Quit | `2cf5cc25e8bf` |
| `src/kept.rs` | `quit_without_windows` | Left out: it quits only when no window is open | `a0e0b218ef71` |
| `src/kept.rs` | `close_textless` | Left out: it closes Settings | `cbda49437f0b` |
| `src/app.rs` | `on_editor_event` | Type | `be9c651e2b04` |
| `src/app.rs` | `edit_list` | Type | `d0a27a483246` |
| `src/app.rs` | `flush` | Flush, Flushed, FlushFails | `fca1bc6bfeaf` |
| `src/app.rs` | `record_save` | Saved, SavedNothing, CreateMove | `dc99e76dcf49` |
| `src/app.rs` | `select` | Select, Flushed | `17119680eb90` |
| `src/app.rs` | `show_list` | LoseFocus | `dc356a9290f0` |
| `src/app.rs` | `open` | Open | `5955c840616a` |
| `src/app.rs` | `replace_editor` | Open | `a27183e2caf5` |
| `src/app.rs` | `on_idle` | IdleMove | `e6abeea85f3d` |
| `src/app.rs` | `on_activation` | LoseFocus, Sync | `d337ffe4d03f` |
| `src/app.rs` | `loaded_now` | Open, RefreshNote | `839318b7a1a9` |
| `src/app.rs` | `sync_with_disk` | Sync | `99f6dd0ecf09` |
| `src/app.rs` | `watch_notes` | Sync | `9a0f92c47a15` |
| `src/app.rs` | `refresh_open_note` | RefreshOpenNote, RefreshNote | `d11ba63e9cb7` |
| `src/app.rs` | `show_in_place` | ShowInPlace | `f0c5ce121cac` |
| `src/app.rs` | `may_leave` | Quit, FlushFails | `cde3598f99a4` |
| `src/app.rs` | `ask_to_quit` | Quit | `5c63262aced4` |
| `src/app.rs` | `quit` | Quit | `9bf55810ea6d` |
| `src/app.rs` | `close` | Quit | `d5edd27afc85` |
| `src/app.rs` | `on_close` | Quit | `21aa9964281f` |
| `src/app.rs` | `step` | Select | `c039d514a3a6` |
| `src/app.rs` | `move_to_trash` | MoveToTrash, Flushed, TrashMove | `f08dcc893178` |
| `src/app.rs` | `show_notes` | ShowNotes | `dd56528f935d` |
| `src/app.rs` | `show_trash` | ShowTrash, Flushed | `777c73bc1331` |
| `src/app.rs` | `show_next_trashed` | ShowNextTrashed | `76a31ce6c5c9` |
| `src/app.rs` | `restore` | Restore | `79882eef2106` |
| `src/app.rs` | `ask_to_delete_forever` | Ask | `b669c2862166` |
| `src/app.rs` | `ask` | Ask | `74ad53d8fabf` |
| `src/app.rs` | `answer` | Cancel, Confirm, PurgeUnlink | `fec7149f72c2` |
| `src/app.rs` | `set_notes_dir` | Select, as leaving a note for a blank one | `d99638780c0e` |
| `src/app.rs` | `new` | Init, and the hooks that call `flush` and `on_close` | `876ac63f5db9` |
| `src/app.rs` | `init` | Left out: it quits only when no window is open | `8d6cb3edca39` |
| `src/app.rs` | `open_filter` | Left out: it sets the text of Find, not of a note | `37f59b12cb50` |
| `src/app.rs` | `reload_lists` | Left out: it fills the lists, and the model has none | `9735f25930f9` |
| `Cargo.lock` | `gpui-base 0.9.0` | Type, Compose, Unmark and ShowInPlace: every edit of the text notifies, which `on_editor_notify` relies on | |
