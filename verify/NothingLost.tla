---------------------------- MODULE NothingLost ----------------------------
(***************************************************************************)
(* Brouyon saving notes into a folder that another program also changes.   *)
(*                                                                         *)
(* Brouyon runs its handlers on one thread, so they never overlap. Another *)
(* program, such as Dropbox, can act between any two of the system calls a *)
(* handler makes. So each step below is one system call of Brouyon's, with *)
(* the plain code that leads up to the next one, or one act of the other   *)
(* program. Names are those of src/app.rs and src/store.rs.                *)
(*                                                                         *)
(* README.md says which function each action stands for.                   *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS
    Names,          \* what a note's file can be called
    Texts,          \* what a note can hold, besides nothing
    None,           \* no file, or no note
    Empty,          \* no text
    Broken          \* which deliberate mistakes are in the code, for testing

Contents == Texts \cup {Empty}
Files    == [Names -> Contents \cup {None}]

VARIABLES
    \* The disk.
    notes,          \* the notes folder
    trash,          \* its .trash
    taken,          \* a `Taken`: a note's file under its temporary name
    \* The fields of `Brouyon` that decide what becomes of the text.
    selection,
    text,           \* what the editor holds
    marked,         \* some of it is still being composed
    dirty,          \* SaveState isn't Kept
    loaded,
    saveFailed,     \* SaveState is Refused
    asking,         \* `confirm` holds a DeleteForever
    \* Where the handler that is running has got to.
    pc, handler,
    \* What the promise is about. Brouyon's code can't see these.
    shown,          \* what the open note's file held when the user last saw it
    overwritten,    \* the acts that destroyed text the user hadn't seen
    dropped         \* the acts that let go of text that wasn't on disk

disk   == <<notes, trash, taken>>
app    == <<selection, text, marked, dirty, loaded, saveFailed, asking>>
run    == <<pc, handler>>
ghosts == <<shown, overwritten, dropped>>
vars   == <<disk, app, run, ghosts>>

Blank      == [kind |-> "blank", id |-> None]
Note(n)    == [kind |-> "note", id |-> n]
Trashed(n) == [kind |-> "trashed", id |-> n]
Selections == {Blank} \cup {Note(n) : n \in Names} \cup {Trashed(n) : n \in Names}

\* What a handler goes on to do once `flush` has saved the text.
NoHandler == [does |-> "nothing", target |-> Blank, from |-> Blank, warned |-> FALSE]
Does(what) == [NoHandler EXCEPT !.does = what]

Init ==
    /\ notes = [n \in Names |-> None]
    /\ trash = [n \in Names |-> None]
    /\ taken = None
    /\ selection = Blank /\ text = Empty /\ marked = FALSE
    /\ dirty = FALSE /\ loaded = None /\ saveFailed = FALSE /\ asking = FALSE
    /\ pc = "ready" /\ handler = NoHandler
    /\ shown = None /\ overwritten = {} /\ dropped = {}

-----------------------------------------------------------------------------
(* Shared by the steps.                                                    *)

\* The user's text that is in the editor and in no file. A trashed note can't
\* be typed in, and an emptied one has no text to lose.
Unsaved ==
    /\ selection.kind # "trashed" /\ text \notin {Empty, shown}
    /\ \A n \in Names : text \notin {notes[n], trash[n]}

\* A step that takes the text out of the editor.
LetsGo(act) == dropped' = IF Unsaved THEN dropped \cup {act} ELSE dropped

\* A step that destroys what a file holds. `fine` is what it may hold.
Destroys(act, held, fine) ==
    overwritten' = IF held \in fine \cup {None, Empty} THEN overwritten
                   ELSE overwritten \cup {act}

Done == pc' = "ready" /\ handler' = NoHandler

\* move_to_free_name takes the name it was given unless a file has it.
FreeNames(folder, wanted) ==
    IF wanted \in Names /\ folder[wanted] = None THEN {wanted}
    ELSE {n \in Names : folder[n] = None}

\* Taken::keep, which goes by move_to_free_name.
KeptNames(folder, wanted) ==
    IF "keep-takes-any-name" \in Broken THEN Names ELSE FreeNames(folder, wanted)

Ready == pc = "ready"
\* What the user does waits for a question on screen. It doesn't wait for a
\* composition to end.
CanAct == Ready /\ ~asking

-----------------------------------------------------------------------------
(* The user's typing.                                                      *)

\* on_editor_notify finds the text changed. Also undo, and the commit of composed text.
Type(t) ==
    /\ Ready /\ ~asking /\ selection.kind # "trashed" /\ t # text
    /\ text' = t /\ dirty' = TRUE /\ marked' = FALSE
    /\ UNCHANGED <<disk, selection, loaded, saveFailed, asking, run, ghosts>>

\* replace_and_mark_text_in_range: the text changes with no InputEvent::Change, and
\* on_editor_notify finds that it did.
Compose(t) ==
    /\ Ready /\ ~asking /\ selection.kind # "trashed" /\ t # text
    /\ text' = t /\ marked' = TRUE
    /\ dirty' = IF "composing-goes-unnoticed" \in Broken THEN dirty ELSE TRUE
    /\ UNCHANGED <<disk, selection, loaded, saveFailed, asking, run, ghosts>>

\* unmark_text: the composed text stays as it is.
Unmark ==
    /\ Ready /\ marked
    /\ marked' = FALSE
    /\ UNCHANGED <<disk, selection, text, dirty, loaded, saveFailed, asking, run, ghosts>>

-----------------------------------------------------------------------------
(* `flush`, from its start to its first system call. Every way of leaving  *)
(* a note starts here.                                                     *)

Flush(h) ==
    /\ handler' = h
    /\ UNCHANGED <<disk, selection, text, marked, loaded, asking, ghosts>>
    /\ IF ~dirty \/ (h.does = "quit" /\ "quit-without-saving" \in Broken)
       THEN pc' = "flushed" /\ UNCHANGED <<dirty, saveFailed>>
       ELSE IF selection.kind = "note" /\ loaded # text
       THEN /\ pc' = IF text = Empty THEN "remove.read" ELSE "save.read"
            /\ UNCHANGED <<dirty, saveFailed>>
       ELSE IF selection = Blank /\ text # Empty
       THEN pc' = "create.move" /\ UNCHANGED <<dirty, saveFailed>>
       ELSE pc' = "flushed" /\ dirty' = FALSE /\ saveFailed' = FALSE

\* The timer of on_editor_notify, 400 ms after the last change.
SaveTimer == Ready /\ dirty /\ Flush(Does("refresh"))

\* on_activation, when the window loses the focus.
LoseFocus == Ready /\ Flush(Does("stay"))

\* select, for a click in the list, the arrows or Ctrl+N.
Select(target) ==
    /\ CanAct
    /\ target.kind = "trashed" <=> selection.kind = "trashed"
    /\ Flush([Does("open") EXCEPT !.target = target])

\* on_idle, which can come while text is being composed.
IdleMove ==
    /\ Ready /\ ~asking /\ ~(selection = Blank /\ ~dirty)
    /\ Flush([Does("open") EXCEPT !.target = Blank])

\* show_trash and, on the way back, show_notes.
ShowTrash ==
    /\ CanAct /\ selection.kind # "trashed"
    /\ Flush(Does("showTrash"))

ShowNotes(target) ==
    /\ CanAct /\ selection.kind = "trashed" /\ target.kind # "trashed"
    /\ Flush([Does("open") EXCEPT !.target = target])

\* move_to_trash.
MoveToTrash ==
    /\ CanAct /\ selection.kind # "trashed"
    /\ Flush([Does("trash") EXCEPT !.from = selection])

\* ask_to_quit, quit, close and on_close, through may_leave.
Quit ==
    /\ CanAct
    /\ Flush([Does("quit") EXCEPT !.warned =
                 saveFailed \/ "any-problem-counts-as-a-warning" \in Broken])

-----------------------------------------------------------------------------
(* The system calls of `flush`.                                            *)

Saved ==
    /\ dirty' = FALSE /\ saveFailed' = FALSE
    /\ loaded' = text /\ shown' = text

SavedNothing ==
    /\ dirty' = FALSE /\ saveFailed' = FALSE
    /\ selection' = Blank /\ loaded' = None /\ shown' = None

\* Store::save asks what the file holds, once the text is in a temporary file.
SaveRead ==
    /\ pc = "save.read"
    /\ LET held == notes[selection.id]
           skips == "save-skips-its-check" \in Broken
       IN  IF held = text
           THEN /\ pc' = "flushed" /\ Saved
                /\ UNCHANGED <<disk, selection, text, marked, asking, handler,
                               overwritten, dropped>>
           ELSE IF held \in {loaded, None} \/ skips
           THEN pc' = "save.put" /\ UNCHANGED <<disk, app, handler, ghosts>>
           ELSE IF "conflict-takes-theirs" \in Broken
           THEN /\ pc' = "flushed" /\ dirty' = FALSE /\ saveFailed' = FALSE
                /\ text' = held /\ loaded' = held /\ shown' = held
                /\ LetsGo("conflict")
                /\ UNCHANGED <<disk, selection, marked, asking, handler, overwritten>>
           \* Saved::Changed: the text goes into a new note beside the file.
           ELSE pc' = "create.move" /\ UNCHANGED <<disk, app, handler, ghosts>>

\* Staged::put swaps the temporary file with the note's in one system call. Then
\* Taken::drop_if_seen reads what came out, where no other program writes.
SavePut ==
    /\ pc = "save.put"
    /\ LET held == notes[selection.id]
           blind == Broken \cap {"save-skips-its-check", "save-in-two-calls"} # {}
       IN  IF held = None
           \* There is nothing to swap with, so move_new is next.
           THEN pc' = "save.move" /\ UNCHANGED <<disk, app, handler, ghosts>>
           ELSE /\ notes' = [notes EXCEPT ![selection.id] = text]
                /\ IF held \in {loaded, text} \/ blind
                   THEN /\ Destroys("save", held, {shown, text})
                        /\ pc' = "flushed" /\ Saved
                        /\ UNCHANGED <<trash, taken, selection, text, marked, asking,
                                       handler, dropped>>
                   ELSE /\ taken' = held /\ pc' = "save.back"
                        /\ UNCHANGED <<trash, app, handler, ghosts>>

\* move_new, for a note that was deleted elsewhere. It refuses a name that
\* something has again, and then FlushFails is the only step.
SaveMove ==
    /\ pc = "save.move" /\ notes[selection.id] = None
    /\ notes' = [notes EXCEPT ![selection.id] = text]
    /\ pc' = "flushed" /\ Saved
    /\ UNCHANGED <<trash, taken, selection, text, marked, asking, handler,
                   overwritten, dropped>>

\* The file had changed after the look. The same swap puts it back, and what
\* comes out is the text just written, unless the file changed once more.
\* Either way it is Saved::Changed, and `flush` goes on to Store::create.
SaveBack ==
    /\ pc = "save.back"
    /\ LET there == notes[selection.id] IN
       \/ /\ there # None
          /\ notes' = [notes EXCEPT ![selection.id] = taken]
          /\ IF there \in {loaded, text} \/ "put-back-drops-what-comes-out" \in Broken
             THEN /\ Destroys("save", there, {shown, text})
                  /\ taken' = None /\ pc' = "create.move"
                  /\ UNCHANGED <<trash, app, handler, shown, dropped>>
             ELSE /\ taken' = there /\ pc' = "save.keep"
                  /\ UNCHANGED <<trash, app, handler, ghosts>>
       \* The swap fails, as it does when the note is gone by now.
       \/ pc' = "save.keep" /\ UNCHANGED <<disk, app, handler, ghosts>>

\* Taken::keep, for whichever file the save still holds.
SaveKeep ==
    /\ pc = "save.keep"
    /\ \E n \in KeptNames(notes, selection.id) :
          /\ notes' = [notes EXCEPT ![n] = taken]
          /\ Destroys("save", notes[n], {shown, text})
    /\ taken' = None /\ pc' = "create.move"
    /\ UNCHANGED <<trash, app, handler, shown, dropped>>

\* Store::remove_unchanged, for a note that was emptied.
RemoveRead ==
    /\ pc = "remove.read"
    /\ LET held == notes[selection.id] IN
       IF held = loaded \/ (held # None /\ "remove-skips-its-check" \in Broken)
       THEN pc' = "remove.take" /\ UNCHANGED <<disk, app, handler, ghosts>>
       ELSE \* Gone already, or KeptTheirs.
            /\ pc' = "flushed" /\ SavedNothing
            /\ UNCHANGED <<disk, text, marked, asking, handler, overwritten, dropped>>

\* Taken::take moves the file to its temporary name in one system call. Then
\* drop_if_seen reads it there.
RemoveTake ==
    /\ pc = "remove.take"
    /\ LET held == notes[selection.id]
           blind == Broken \cap {"remove-skips-its-check", "remove-in-two-calls"} # {}
       IN  IF held = None
           THEN /\ pc' = "flushed" /\ SavedNothing
                /\ UNCHANGED <<disk, text, marked, asking, handler, overwritten, dropped>>
           ELSE /\ notes' = [notes EXCEPT ![selection.id] = None]
                /\ IF held = loaded \/ blind
                   THEN /\ Destroys("remove", held, {shown})
                        /\ pc' = "flushed" /\ SavedNothing
                        /\ UNCHANGED <<trash, taken, text, marked, asking, handler, dropped>>
                   ELSE /\ taken' = held /\ pc' = "remove.keep"
                        /\ UNCHANGED <<trash, app, handler, ghosts>>

\* Taken::keep puts back the file that had changed after the look, under its
\* own name unless something has it by now. Removed::Changed.
RemoveKeep ==
    /\ pc = "remove.keep"
    /\ \E n \in KeptNames(notes, selection.id) :
          /\ notes' = [notes EXCEPT ![n] = taken]
          /\ Destroys("remove", notes[n], {shown})
    /\ taken' = None
    /\ pc' = "flushed" /\ SavedNothing
    /\ UNCHANGED <<trash, text, marked, asking, handler, dropped>>

\* Store::create, after the fsync: move_to_free_name.
CreateMove ==
    /\ pc = "create.move"
    /\ \E n \in IF "create-takes-any-name" \in Broken THEN Names
                ELSE FreeNames(notes, None) :
          /\ notes' = [notes EXCEPT ![n] = text]
          /\ Destroys("create", notes[n], {text})
          /\ selection' = Note(n)
    /\ pc' = "flushed" /\ Saved
    /\ UNCHANGED <<trash, taken, text, marked, asking, handler, dropped>>

\* A system call fails, such as on a full disk. The text stays in the editor.
\* A `Taken` that can't be kept stays under its temporary name, where nothing
\* writes over it, and the model follows it no further: README.md has it.
FlushFails ==
    /\ pc \in {"save.read", "save.put", "save.move", "save.keep", "create.move",
               "remove.read", "remove.take", "remove.keep"}
    /\ saveFailed' = TRUE
    /\ dirty' = ("failed-save-counts-as-saved" \notin Broken)
    /\ taken' = None
    /\ UNCHANGED <<notes, trash, selection, text, marked, loaded, asking, shown,
                   overwritten>>
    /\ IF /\ handler.does = "quit"
          /\ handler.warned \/ "quit-ignores-a-failed-save" \in Broken
       THEN /\ pc' = "closed" /\ handler' = NoHandler
            \* A save that failed before, and said so, lets the text go.
            /\ IF saveFailed THEN UNCHANGED dropped ELSE LetsGo("quit")
       ELSE /\ UNCHANGED dropped
            /\ IF handler.does = "open" /\ "select-ignores-a-failed-save" \in Broken
               THEN pc' = "open" /\ UNCHANGED handler
               ELSE pc' = "ready" /\ handler' = NoHandler

-----------------------------------------------------------------------------
(* Once `flush` has saved the text: the rest of the handler that called it. *)

\* show_in_place, for the open note. Called by refresh_open_note.
ShowInPlace(t) == text' = t /\ marked' = FALSE /\ LetsGo("refresh")

\* refresh_open_note, in full. It reads one file.
RefreshNote ==
    LET held == notes[selection.id] IN
    /\ UNCHANGED <<disk, dirty, saveFailed, asking, overwritten>>
    /\ IF held = None
       THEN selection' = Blank /\ loaded' = None /\ shown' = None /\ ShowInPlace(Empty)
       ELSE /\ UNCHANGED selection
            /\ loaded' = held
            /\ IF loaded # held /\ "refresh-only-remembers" \notin Broken
               THEN shown' = held /\ ShowInPlace(held)
               ELSE UNCHANGED <<shown, text, marked, dropped>>

\* A trashed note that has gone gives way to the next one, or to the notes.
AfterTrashed ==
    LET left == {n \in Names : trash[n] # None} IN
    IF left # {} THEN {Trashed(n) : n \in left}
    ELSE {Blank} \cup {Note(n) : n \in {m \in Names : notes[m] # None}}

ShowNextTrashed ==
    \E next \in AfterTrashed :
        /\ handler' = [Does("open") EXCEPT !.target = next]
        /\ pc' = "open"
       

RefreshOpenNote ==
    IF selection.kind = "note" /\ (~dirty \/ "refresh-ignores-dirty" \in Broken)
    THEN RefreshNote /\ Done
    ELSE IF selection.kind = "trashed" /\ ~asking /\ trash[selection.id] = None
    THEN ShowNextTrashed /\ UNCHANGED <<disk, app, ghosts>>
    ELSE Done /\ UNCHANGED <<disk, app, ghosts>>

Flushed ==
    /\ pc = "flushed"
    /\ CASE handler.does = "stay" ->
              Done /\ UNCHANGED <<disk, app, ghosts>>
         [] handler.does = "refresh" ->
              RefreshOpenNote
         \* Reselecting the open note keeps its editor.
         [] handler.does = "open" ->
              IF handler.target = selection /\ selection # Blank
              THEN Done /\ UNCHANGED <<disk, app, ghosts>>
              ELSE pc' = "open" /\ UNCHANGED <<disk, app, handler, ghosts>>
         [] handler.does = "showTrash" ->
              IF \E n \in Names : trash[n] # None
              THEN \E n \in Names :
                      /\ trash[n] # None
                      /\ pc' = "open"
                      /\ handler' = [Does("open") EXCEPT !.target = Trashed(n)]
                      /\ UNCHANGED <<disk, app, ghosts>>
              ELSE Done /\ UNCHANGED <<disk, app, ghosts>>
         \* The save kept both, and neither goes without a look.
         [] handler.does = "trash" ->
              IF \/ selection.kind # "note"
                 \/ handler.from.kind = "note" /\ handler.from # selection
              THEN Done /\ UNCHANGED <<disk, app, ghosts>>
              ELSE pc' = "trash.move" /\ UNCHANGED <<disk, app, handler, ghosts>>
         [] handler.does = "quit" ->
              /\ pc' = "closed" /\ handler' = NoHandler
              /\ LetsGo("quit")
              /\ UNCHANGED <<disk, app, shown, overwritten>>

\* `open` reads the file and shows it in a new editor. A file that can't be
\* read leaves the note that was open where it is.
Open ==
    /\ pc = "open"
    /\ LET target == handler.target
           held == CASE target.kind = "blank" -> Empty
                     [] target.kind = "note" -> notes[target.id]
                     [] target.kind = "trashed" -> trash[target.id]
       IN  IF held = None
           THEN UNCHANGED <<disk, app, ghosts>>
           ELSE /\ LetsGo("open")
                /\ selection' = target /\ text' = held /\ marked' = FALSE
                /\ loaded' = IF target.kind = "note" THEN held ELSE None
                /\ shown' = IF target.kind = "blank" THEN None ELSE held
                /\ UNCHANGED <<disk, dirty, saveFailed, asking, overwritten>>
    /\ Done

-----------------------------------------------------------------------------
(* The trash.                                                              *)

\* Store::trash, which moves the file, whatever it holds by then.
TrashMove ==
    /\ pc = "trash.move"
    /\ IF notes[selection.id] = None
       THEN Done /\ UNCHANGED <<disk, app, ghosts>>
       ELSE \E n \in IF "trash-takes-any-name" \in Broken THEN Names
                     ELSE FreeNames(trash, selection.id) :
            /\ trash' = [trash EXCEPT ![n] = notes[selection.id]]
            /\ notes' = [notes EXCEPT ![selection.id] = None]
            /\ Destroys("trash", trash[n], {notes[selection.id]})
            /\ pc' = "open"
            /\ handler' = [Does("open") EXCEPT !.target = Blank]
            /\ UNCHANGED <<taken, app, shown, dropped>>

\* Store::restore.
Restore ==
    /\ CanAct /\ selection.kind = "trashed"
    /\ IF trash[selection.id] = None
       THEN UNCHANGED vars
       ELSE \E n \in IF "restore-takes-any-name" \in Broken THEN Names
                     ELSE FreeNames(notes, selection.id) :
            /\ notes' = [notes EXCEPT ![n] = trash[selection.id]]
            /\ trash' = [trash EXCEPT ![selection.id] = None]
            /\ Destroys("restore", notes[n], {trash[selection.id]})
            /\ pc' = "open"
            /\ handler' = [Does("open") EXCEPT !.target = Note(n)]
            /\ UNCHANGED <<taken, app, shown, dropped>>

\* Delete forever asks first. Emptying the trash is left out: whoever confirms
\* that asks for every note in it to go, seen or not.
Ask ==
    /\ CanAct /\ selection.kind = "trashed"
    /\ asking' = TRUE
    /\ UNCHANGED <<disk, selection, text, marked, dirty, loaded, saveFailed, run, ghosts>>

Cancel ==
    /\ Ready /\ asking
    /\ asking' = FALSE
    /\ UNCHANGED <<disk, selection, text, marked, dirty, loaded, saveFailed, run, ghosts>>

\* `answer`, up to the first read of Store::purge.
Confirm ==
    /\ Ready /\ asking
    /\ asking' = FALSE
    /\ UNCHANGED <<disk, selection, text, marked, dirty, loaded, saveFailed, ghosts>>
    /\ IF trash[selection.id] = None
       THEN ShowNextTrashed
       ELSE IF trash[selection.id] = shown \/ "purge-skips-its-check" \in Broken
       THEN pc' = "purge.take" /\ UNCHANGED handler
       \* It wasn't deleted, and is shown as it is now.
       ELSE /\ pc' = "open"
            /\ handler' = [Does("open") EXCEPT !.target = selection]

\* Taken::take and drop_if_seen, as for a note that was emptied.
PurgeTake ==
    /\ pc = "purge.take"
    /\ LET held == trash[selection.id]
           blind == Broken \cap {"purge-skips-its-check", "purge-in-two-calls"} # {}
       IN  IF held = None
           THEN ShowNextTrashed /\ UNCHANGED <<disk, app, ghosts>>
           ELSE /\ trash' = [trash EXCEPT ![selection.id] = None]
                /\ IF held = shown \/ blind
                   THEN /\ Destroys("purge", held, {shown})
                        /\ LET left == {n \in Names : trash'[n] # None} IN
                           \E next \in IF left # {} THEN {Trashed(n) : n \in left}
                                       ELSE {Blank} \cup {Note(n) : n \in {m \in Names : notes[m] # None}} :
                              /\ handler' = [Does("open") EXCEPT !.target = next]
                              /\ pc' = "open"
                        /\ UNCHANGED <<notes, taken, app, shown, dropped>>
                   ELSE /\ taken' = held /\ pc' = "purge.keep"
                        /\ UNCHANGED <<notes, app, handler, ghosts>>

\* Taken::keep. Removed::Changed: it wasn't deleted, and is shown as it is now.
PurgeKeep ==
    /\ pc = "purge.keep"
    /\ \E n \in KeptNames(trash, selection.id) :
          /\ trash' = [trash EXCEPT ![n] = taken]
          /\ Destroys("purge", trash[n], {shown})
    /\ taken' = None
    /\ pc' = "open"
    /\ handler' = [Does("open") EXCEPT !.target = selection]
    /\ UNCHANGED <<notes, app, shown, dropped>>

-----------------------------------------------------------------------------
(* sync_with_disk, for the folder's watcher and when the window gets the   *)
(* focus.                                                                  *)

Sync == Ready /\ RefreshOpenNote

-----------------------------------------------------------------------------
(* The other program: Dropbox, an editor, another computer by way of it.   *)
(* It replaces whole files at once, as a sync client does, between any two  *)
(* of Brouyon's system calls. It leaves the temporary names alone.         *)

TheyMayAct == pc # "closed"

TheyWrite(n, t) ==
    /\ TheyMayAct /\ notes[n] # t
    /\ notes' = [notes EXCEPT ![n] = t]
    /\ UNCHANGED <<trash, taken, app, run, ghosts>>

TheyDelete(n) ==
    /\ TheyMayAct /\ notes[n] # None
    /\ notes' = [notes EXCEPT ![n] = None]
    /\ UNCHANGED <<trash, taken, app, run, ghosts>>

TheyWriteInTrash(n, t) ==
    /\ TheyMayAct /\ trash[n] # t
    /\ trash' = [trash EXCEPT ![n] = t]
    /\ UNCHANGED <<notes, taken, app, run, ghosts>>

TheyDeleteFromTrash(n) ==
    /\ TheyMayAct /\ trash[n] # None
    /\ trash' = [trash EXCEPT ![n] = None]
    /\ UNCHANGED <<notes, taken, app, run, ghosts>>

Closed == pc = "closed" /\ UNCHANGED vars

-----------------------------------------------------------------------------
Next ==
    \/ \E t \in Contents : Type(t)
    \/ \E t \in Texts : Compose(t)
    \/ Unmark
    \/ SaveTimer
    \/ LoseFocus
    \/ \E target \in Selections : Select(target)
    \/ IdleMove
    \/ ShowTrash
    \/ \E target \in Selections : ShowNotes(target)
    \/ MoveToTrash
    \/ Quit
    \/ SaveRead
    \/ SavePut
    \/ SaveMove
    \/ SaveBack
    \/ SaveKeep
    \/ RemoveRead
    \/ RemoveTake
    \/ RemoveKeep
    \/ CreateMove
    \/ FlushFails
    \/ Flushed
    \/ Open
    \/ TrashMove
    \/ Restore
    \/ Ask
    \/ Cancel
    \/ Confirm
    \/ PurgeTake
    \/ PurgeKeep
    \/ Sync
    \/ \E n \in Names, t \in Contents : TheyWrite(n, t)
    \/ \E n \in Names : TheyDelete(n)
    \/ \E n \in Names, t \in Contents : TheyWriteInTrash(n, t)
    \/ \E n \in Names : TheyDeleteFromTrash(n)
    \/ Closed

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* The promise of DOC.md, "Brouyon never writes over a change it hasn't    *)
(* shown you" and "nothing is lost", one rule for each act that could      *)
(* break it, so that a broken rule says where to look.                     *)

\* Text that another program wrote, and the user hasn't seen, is still there.
SaveKeepsTheirs    == "save"    \notin overwritten
RemoveKeepsTheirs  == "remove"  \notin overwritten
CreateKeepsTheirs  == "create"  \notin overwritten
TrashKeepsTheirs   == "trash"   \notin overwritten
RestoreKeepsTheirs == "restore" \notin overwritten
PurgeKeepsTheirs   == "purge"   \notin overwritten

\* Text the user typed leaves the editor only once it is in a file, or once the
\* user has been told that it couldn't be saved.
LeavingKeepsMine   == "open"     \notin dropped
RefreshKeepsMine   == "refresh"  \notin dropped
ConflictKeepsMine  == "conflict" \notin dropped
QuittingKeepsMine  == "quit"     \notin dropped

\* Not part of the promise: what `loaded` is for. It has to be what the user saw.
LoadedIsShown == selection.kind = "note" => loaded = shown

\* SaveState has three values, so a text can't be both refused and kept.
RefusedIsPending == saveFailed => dirty

TypeOK ==
    /\ notes \in Files /\ trash \in Files
    /\ taken \in Contents \cup {None}
    /\ selection \in Selections
    /\ text \in Contents /\ marked \in BOOLEAN /\ dirty \in BOOLEAN
    /\ loaded \in Contents \cup {None} /\ saveFailed \in BOOLEAN
    /\ asking \in BOOLEAN
    /\ shown \in Contents \cup {None}

\* A `Taken` is dropped or kept before its handler ends.
TakenIsMidway ==
    taken # None => pc \in {"save.back", "save.keep", "remove.keep", "purge.keep"}

\* A trashed note holds no text of the user's, so nothing is pending there.
TrashIsReadOnly == selection.kind = "trashed" => ~dirty /\ ~marked

Symmetry == Permutations(Names) \cup Permutations(Texts)
=============================================================================
