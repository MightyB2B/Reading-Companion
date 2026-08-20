import { useCallback, useEffect, useRef, useState } from "react";
import { api, type Move, type Note, type StoredAnchor } from "../../lib/api";
import { pageLabel } from "../../lib/selection";
import type { FlowSelection } from "../Flow";
import {
  EmptyState,
  MoveChip,
  PanelHeader,
  useMoveCatalogue,
} from "../PanelChrome";

/**
 * Notes and marks.
 *
 * A note is tied to where it was needed — the book, the block, and the exact
 * characters that were selected — and it survives a re-transcription by
 * carrying its own anchor text.
 *
 * The body never renders in the reading column. The page stays the page: all
 * the text shows is a dot in the gutter saying something is there. This panel
 * and the text point at each other; neither is pasted into the other.
 */
export function NotesPanel({
  bookId,
  bookTitle,
  focusSignal,
  selection,
  revision,
  onChanged,
  onGoTo,
}: {
  bookId: number;
  /** Named in the header, so which book these belong to is never a guess. */
  bookTitle: string;
  /** Bumped when the toolbar sent us here to write something. */
  focusSignal: number;
  /** What the reader has highlighted in the text right now. */
  selection: FlowSelection | null;
  revision: number;
  onChanged: () => void;
  onGoTo: (blockId: number) => void;
}) {
  const [notes, setNotes] = useState<Note[]>([]);
  const [draft, setDraft] = useState("");
  const [draftMove, setDraftMove] = useState<Move | null>(null);
  const [editing, setEditing] = useState<number | null>(null);
  const [editBody, setEditBody] = useState("");
  const [error, setError] = useState<string | null>(null);
  /** Anchors for whichever note has been expanded. */
  const [anchors, setAnchors] = useState<Record<number, StoredAnchor[]>>({});
  const box = useRef<HTMLTextAreaElement>(null);
  const moves = useMoveCatalogue();
  /**
   * Whether the compose form is open.
   *
   * Closed by default so the panel opens on the notes themselves. An empty
   * three-field form above your actual work is the wrong way round: the list
   * is consulted constantly, the form used occasionally.
   */
  const [composing, setComposing] = useState(false);
  /** The nine move chips, only while actually tagging. */
  const [tagging, setTagging] = useState(false);

  // Arriving from the toolbar's Note button opens the form and takes the
  // cursor — the whole point of that button is to skip these two steps.
  useEffect(() => {
    if (focusSignal > 0) {
      setComposing(true);
      requestAnimationFrame(() => box.current?.focus());
    }
  }, [focusSignal]);

  // A fresh selection is a strong hint that something is about to be written.
  useEffect(() => {
    if (selection) setComposing(true);
  }, [selection]);

  /** Show or hide where a note is attached. */
  const toggleAnchors = async (noteId: number) => {
    if (anchors[noteId]) {
      setAnchors((a) => {
        const next = { ...a };
        delete next[noteId];
        return next;
      });
      return;
    }
    const list = await api.noteAnchors(noteId);
    setAnchors((a) => ({ ...a, [noteId]: list }));
  };

  /**
   * Attach the current selection to a note that already exists.
   *
   * The reason a note takes several anchors: a theme turns up in a dozen
   * places, and writing the same paragraph of thought twelve times is how
   * people stop taking notes.
   */
  const attachHere = async (noteId: number) => {
    if (!selection) return;
    setError(null);
    try {
      await api.attachAnchors(
        noteId,
        selection.anchors.map((a) => ({
          block_id: a.blockId,
          char_start: a.charStart,
          char_end: a.charEnd,
          anchor_text: a.text,
        })),
      );
      setAnchors((a) => {
        const next = { ...a };
        delete next[noteId];
        return next;
      });
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const refresh = useCallback(() => {
    api.listNotes(bookId).then(setNotes).catch((e) => setError(String(e)));
  }, [bookId]);

  useEffect(refresh, [refresh, revision]);

  const save = async () => {
    setError(null);
    try {
      // A selection can span several paragraphs. The first anchor is the
      // note's primary one; the rest are attached alongside it, so a note
      // written about an argument running across a page boundary stays
      // attached to all of it.
      await api.addNote({
        bookId,
        blockId: selection?.anchors[0]?.blockId ?? null,
        charStart: selection?.anchors[0]?.charStart ?? null,
        charEnd: selection?.anchors[0]?.charEnd ?? null,
        anchorText: selection?.anchors[0]?.text ?? "",
        extraAnchors: (selection?.anchors ?? []).slice(1).map((a) => ({
          block_id: a.blockId,
          char_start: a.charStart,
          char_end: a.charEnd,
          anchor_text: a.text,
        })),
        move: draftMove,
        body: draft,
      });
      setDraft("");
      setDraftMove(null);
      refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const remove = async (id: number) => {
    await api.deleteNote(id);
    refresh();
    onChanged();
  };

  const commitEdit = async (id: number, move: Move | null) => {
    await api.updateNote(id, editBody, move);
    setEditing(null);
    refresh();
    onChanged();
  };

  return (
    <div>
      <PanelHeader
        title={`Notes · ${bookTitle}`}
        purpose="Anything you want to say about a passage. Questions count. Confusion counts. Select words in the text first and the note stays tied to them."
        action={
          <button
            onClick={() => setComposing((v) => !v)}
            className="rounded border border-rule px-2 py-1 text-xs hover:border-accent hover:text-accent"
          >
            {composing ? "Close" : "Write one"}
          </button>
        }
      />

      <div className={composing ? "border-b border-rule px-4 py-3" : "hidden"}>
        {selection ? (
          <p className="mb-2 line-clamp-2 rounded bg-paper-dim px-2 py-1 text-xs text-ink-soft">
            on “{selection.text}”
            {pageLabel(selection.pageNos) && ` · ${pageLabel(selection.pageNos)}`}
            {selection.anchors.length > 1 &&
              ` · ${selection.anchors.length} paragraphs`}
          </p>
        ) : (
          <p className="mb-2 text-xs text-ink-soft">
            Nothing selected — this will be a note about the book.
          </p>
        )}

        <textarea
          ref={box}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          rows={3}
          placeholder="What do you want to remember about this?"
          className="w-full resize-y rounded border border-rule bg-transparent px-2 py-1.5 text-sm outline-none focus:border-accent"
        />

        {/* Nine coloured pills wrapped to three rows in a narrow pane, on
            screen permanently, for something needed only at the instant of
            tagging. */}
        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <button
            onClick={() => setTagging((v) => !v)}
            className="text-xs text-ink-soft hover:text-accent"
          >
            {draftMove ? "Tagged:" : "Tag it"} ▾
          </button>
          {draftMove && !tagging && (
            <MoveChip
              move={draftMove}
              info={moves.find((m) => m.id === draftMove)}
              selected
              onClick={() => setTagging(true)}
            />
          )}
          {tagging &&
            moves.map((m) => (
              <MoveChip
                key={m.label}
                move={m.label}
                info={m}
                selected={draftMove === m.id}
                onClick={() => {
                  setDraftMove(draftMove === m.id ? null : m.id);
                  setTagging(false);
                }}
              />
            ))}
        </div>

        <div className="mt-2 flex items-center gap-2">
          <button
            onClick={save}
            disabled={!draft.trim() && !draftMove}
            className="rounded bg-accent px-3 py-1 text-xs font-semibold text-paper disabled:opacity-40"
          >
            Save note
          </button>
          <span className="text-xs text-ink-soft">
            {draftMove && !draft.trim()
              ? "Saves as a highlight with no words."
              : ""}
          </span>
        </div>
        {error && <p className="mt-2 text-xs text-danger">{error}</p>}
      </div>

      {notes.length === 0 ? (
        <EmptyState
          lead="Nothing written yet. A note belongs to the words you had selected, so it is still here when you come back to that paragraph months later."
          example={
            <div className="text-sm">
              <p className="text-xs text-ink-soft">on “that which is in itself”</p>
              <p className="mt-1">
                Is this the same sense as in the definition, or has it shifted?
                Check I P5.
              </p>
            </div>
          }
        />
      ) : (
        <ul className="divide-y divide-rule">
          {notes.map((n) => (
            <li key={n.id} className="group px-4 py-3">
              {n.orphaned && (
                <p className="mb-1 text-xs text-warn">
                  Came loose — the page was re-read and this text moved.
                </p>
              )}
              {n.anchor_text && (
                <button
                  onClick={() => n.block_id && onGoTo(n.block_id)}
                  disabled={!n.block_id}
                  className="mb-1 block max-w-full truncate text-left text-xs text-ink-soft hover:text-accent disabled:hover:text-ink-soft"
                  title="Go to this passage"
                >
                  “{n.anchor_text}”
                </button>
              )}

              {editing === n.id ? (
                <div>
                  <textarea
                    value={editBody}
                    onChange={(e) => setEditBody(e.target.value)}
                    rows={3}
                    className="w-full resize-y rounded border border-accent bg-transparent px-2 py-1.5 text-sm outline-none"
                  />
                  <div className="mt-1 flex gap-2 text-xs">
                    <button
                      onClick={() => commitEdit(n.id, n.move)}
                      className="rounded bg-accent px-2 py-1 text-paper"
                    >
                      Save
                    </button>
                    <button
                      onClick={() => setEditing(null)}
                      className="rounded border border-rule px-2 py-1"
                    >
                      Cancel
                    </button>
                  </div>
                </div>
              ) : (
                <p className="whitespace-pre-wrap text-sm">{n.body}</p>
              )}

              {anchors[n.id] && (
                <ul className="mt-1.5 space-y-1 border-l border-rule pl-2">
                  {anchors[n.id].map((a) => (
                    <li key={a.id} className="flex items-start gap-1.5 text-xs">
                      <button
                        onClick={() => a.block_id && onGoTo(a.block_id)}
                        disabled={!a.block_id}
                        className="min-w-0 flex-1 truncate text-left text-ink-soft hover:text-accent"
                      >
                        {a.page_no != null && (
                          <span className="tabular-nums">p. {a.page_no} · </span>
                        )}
                        “{a.anchor_text}”
                        {a.orphaned && " (came loose)"}
                      </button>
                      <button
                        onClick={async () => {
                          await api.deleteNoteAnchor(a.id);
                          setAnchors((m) => {
                            const next = { ...m };
                            delete next[n.id];
                            return next;
                          });
                          onChanged();
                        }}
                        title="Detach from here"
                        className="flex-none text-ink-soft hover:text-danger"
                      >
                        ✕
                      </button>
                    </li>
                  ))}
                </ul>
              )}

              <div className="mt-1.5 flex items-center gap-2 text-xs opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
                {n.move && (
                  <MoveChip
                    move={n.move}
                    info={moves.find((m) => m.id === n.move)}
                  />
                )}
                <button
                  onClick={() => {
                    setEditing(n.id);
                    setEditBody(n.body);
                  }}
                  className="text-ink-soft hover:text-accent"
                >
                  edit
                </button>
                <button
                  onClick={() => toggleAnchors(n.id)}
                  className="text-ink-soft hover:text-accent"
                >
                  {anchors[n.id] ? "hide places" : "places"}
                </button>
                {selection && (
                  <button
                    onClick={() => attachHere(n.id)}
                    title="Also attach this note to what you have selected"
                    className="text-accent hover:underline"
                  >
                    attach here
                  </button>
                )}
                <button
                  onClick={() => remove(n.id)}
                  className="text-ink-soft hover:text-danger"
                >
                  delete
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
