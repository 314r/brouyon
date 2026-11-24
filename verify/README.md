# Nothing is lost

DOC.md promises that Brouyon never writes over a change it hasn't shown you, and that nothing is lost. Where it breaks, a note is gone with no error and no copy in the Trash.

A test runs one order of events. `NothingLost.tla` describes `flush`, `refresh_open_note`, the trash and `Store` one system call at a time, beside another program that can act between any two of them. TLC tries every order.

## Run it

```sh
verify/check
```

It takes about a minute and a half on 16 cores and needs `mise`, which fetches the Java it pins. The first run downloads TLC 1.7.4 into `target/verify/` and checks its hash. It reads no notes and opens no window. CI runs it.

| Line | What it says |
|---|---|
| `ok promise` | Every rule holds in every state within the bounds. |
| `ok mistake-…` | A mistake was caught. Where a commit is named, the mistake was in the code before it. |
| `WRONG …` | A rule broke, or a mistake got through. |
| `DRIFT …` | The covered code changed since the model was confirmed to describe it. |

It exits with 0 when all is as recorded, 1 for `WRONG`, and 2 for `DRIFT` alone. `verify/check --trace CASE RULE` tells a counterexample in the code's words.

## When the code changes

`verify/check --drift` names each covered function that changed, and each new one that handles notes' text. For each of them:

1. Read its diff against the action the table below gives for it.
2. Make the model do what the code now does. A step is one system call, with the reads of a file that nothing else can reach. The other program acts between any two steps.
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
| The same, between two system calls of `Store` | `at_gap(2, move \|\| …)`, for the second `gap()` it reaches |
| `Sync` | `notice_changes(&view, cx)` |

## Findings

None is open.

**A look and its act were two system calls** (fixed; `mistake-save-in-two-calls`, `mistake-remove-in-two-calls`, `mistake-purge-in-two-calls`). Another program that wrote between the two was overwritten or deleted, in `Store::save`, `remove_unchanged` and `purge`. Now the act comes first and takes nothing away, and the look that decides comes after it:
- `Store::save` swaps its temporary file with the note's in one system call, then reads what came out. If the user hasn't seen it, the same swap puts it back.
- `remove_unchanged` and `purge` move the file to its temporary name in one system call, then read it there. If the user hasn't seen it, it goes back under a name nothing has.

The first look is still there. It leaves a file that changed untouched, which is the usual case.

## What it covers

Bounds: three file names and three texts for `promise`, two names and two or three texts for the rest. The number of steps isn't bounded. Nor is how often the other program acts or a system call fails.

It assumes that:
- Brouyon is the only Brouyon on the folder on this computer. Two would share the temporary file that `temp_name` names.
- The other program replaces whole files at once, as Dropbox does. A file written in place, and read half-written, is left out.
- Handlers run one at a time on the main thread.
- The file system can refuse to replace a file, and can swap two. Where it can't, such as on NFS, `move_new_in_two` and `Staged::replace` look and act in two calls.

It leaves out: several notes folders (`set_notes_dir` counts as leaving a note), Empty Trash, which asks for every note to go, seen or not, files that aren't UTF-8, text that is only white space, dates, the list, power loss, a kill before the save timer, and what Dropbox does with two versions. It also stops following a file that changed and couldn't be put back, because that system call failed too. The file stays under its temporary name, the save says it failed, and `keep_stale` makes it a note before anything writes to that name. After a save that failed, and said so, quitting may drop the text: `may_leave` means it to.

A pass is about this model within these bounds, not the whole app. The table is what ties it to the code.

## Covered code

| File | Function | In the model | Confirmed |
|---|---|---|---|
| `src/store.rs` | `save` | SaveRead, SavePut, SaveMove, SaveBack, SaveKeep | `270daa70e47e` |
| `src/store.rs` | `remove_unchanged` | RemoveRead, RemoveTake, RemoveKeep | `d0c8afa8d446` |
| `src/store.rs` | `create` | CreateMove | `59a1f75500ec` |
| `src/store.rs` | `trash` | TrashMove | `34c47a7e06a4` |
| `src/store.rs` | `restore` | Restore | `997d7de4b075` |
| `src/store.rs` | `purge` | Confirm, PurgeTake, PurgeKeep | `881cfd35ae7b` |
| `src/store.rs` | `remove_if_holds` | RemoveRead, RemoveTake, RemoveKeep, Confirm, PurgeTake, PurgeKeep | `cb2fed549846` |
| `src/store.rs` | `gone` | RemoveRead, RemoveTake, PurgeTake | `f2b029774ab1` |
| `src/store.rs` | `write` | SaveRead, CreateMove: the text is in a temporary file before the first step, and never over a file that is there | `5cfc544a59e7` |
| `src/store.rs` | `put` | SavePut, SaveMove | `ff27658534db` |
| `src/store.rs` | `into_note` | CreateMove | `47000c45ae96` |
| `src/store.rs` | `drop` | FlushFails: a temporary file that wasn't moved goes | `078952d49c2f` |
| `src/store.rs` | `take` | RemoveTake, PurgeTake | `629613436b3a` |
| `src/store.rs` | `drop_if_seen` | SavePut, SaveBack, RemoveTake, PurgeTake | `1c9e6b354587` |
| `src/store.rs` | `keep` | SaveKeep, RemoveKeep, PurgeKeep | `559b6347bfc6` |
| `src/store.rs` | `keep_stale` | The assumption that a file left under a temporary name is never written over | `eedda3c25689` |
| `src/store.rs` | `swap` | SavePut, SaveBack | `983856e53c53` |
| `src/store.rs` | `gap` | TheyMayAct: where the other program acts, between two steps | `574af4729fe8` |
| `src/store.rs` | `replace` | Left out: the settings, and the assumption about the file system | `a8cfe4e7d8c5` |
| `src/store.rs` | `move_to_free_name` | FreeNames | `76ad09b436f2` |
| `src/store.rs` | `move_new` | CreateMove, TrashMove, Restore, SaveMove, RemoveTake, PurgeTake | `d050286bc60f` |
| `src/store.rs` | `move_new_in_two` | Left out: the assumption about the file system | `097c8f1da99a` |
| `src/store.rs` | `empty_trash` | Left out: it asks for every note to go, seen or not | `d99e3e7b6230` |
| `src/store.rs` | `write_atomically` | Left out: it writes the settings | `03f8e04703fe` |
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
| `src/app.rs` | `answer` | Cancel, Confirm, PurgeTake, PurgeKeep | `fec7149f72c2` |
| `src/app.rs` | `set_notes_dir` | Select, as leaving a note for a blank one | `d99638780c0e` |
| `src/app.rs` | `new` | Init, and the hooks that call `flush` and `on_close` | `7ed04ac8791b` |
| `src/app.rs` | `init` | Left out: it quits only when no window is open | `a8f178eebb98` |
| `src/app.rs` | `open_filter` | Left out: it sets the text of Find, not of a note | `37f59b12cb50` |
| `src/app.rs` | `reload_lists` | Left out: it fills the lists, and the model has none | `9735f25930f9` |
| `Cargo.lock` | `gpui-base 0.9.0` | Type, Compose, Unmark and ShowInPlace: every edit of the text notifies, which `on_editor_notify` relies on | |
