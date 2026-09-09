# Explorer Catalog Search: Honest next/prev Keys

**Date:** 2026-09-09  
**Status:** Approved for planning  
**Repo:** local clone of `yelog/lazydb`  
**Goal:** Make the `f` catalog search status hint name keys that actually work, and give match-to-match navigation a binding that does not collide with query text input.

## Problem

The Explorer has two search projections with different key models:

- `/` opens the visible-node find: a two-phase flow (Editing → Enter → Confirmed) whose status hint switches per phase, and whose `n/N` cycle confirmed matches for real (`src/input/keymap.rs:600-614`).
- `f` opens the catalog search: a live filter that never leaves `ExplorerSearchPhase::Editing`. Enter maps to `ExplorerSearchLocate`, which reveals the hit in the tree and closes the search (`src/model/workspace.rs:848`). The Confirmed branch in the keymap (`src/input/keymap.rs:620-635`) is unreachable — `Explorer::confirm_search` (`src/model/workspace.rs:807`) has no caller.

Despite that, the catalog search status line unconditionally renders `"{n} results  n/N next/prev  Enter locate  Esc close"` (`src/ui/mod.rs:2620-2623`). So the hint:

- advertises `n/N`, which never work at any point in this UI (while editing they are inserted as query text), and
- hides `↑/↓`/`Home`/`End`, which do move the selection during editing (`src/input/keymap.rs:654-657`).

## Success criteria

- While catalog search is open, `Ctrl+N` / `Ctrl+P` jump to the next / previous **match row** with wraparound (`move_search_match`), without leaving Editing phase.
- Plain `n` / `N` still insert into the query.
- The status hint names only keys that work in the current UI.
- Help palette rows for catalog search next/prev appear under a context that is actually reachable.
- `/` find behavior and hints are unchanged.
- Keymap and render tests cover the new bindings and hint.

## Non-goals

- Removing the dead Confirmed-phase code for catalog search (`confirm_search`, the unreachable keymap branch, `ExplorerCatalogSearchConfirmed` context). Left as is.
- Changing Enter to confirm-and-stay instead of locate-and-close.
- New configurable keybinding entries in `config/default.toml`.

## Approach

**Fix the Editing phase, where the search actually lives** (chosen over a find-style two-phase rework or hint-only fix). Plain letters cannot be overloaded in a text-input mode, so match navigation moves to `Ctrl+N`/`Ctrl+P` — the same convention the SQL completion popup already uses (`src/input/keymap.rs:837-843`).

## Changes

1. **Keymap — `src/input/keymap.rs`, catalog-search Editing branch (~line 636-660)**  
   After the Ctrl+U clear handler and before the modifier fallthrough guard (`if !event.modifiers.is_empty() … return None`), add exact-match handling:
   - `event.modifiers == KeyModifiers::CONTROL && code == Char('n')` → `Action::ExplorerSearchNext`
   - same for `Char('p')` → `Action::ExplorerSearchPrevious`  
   Existing actions are reused: both already dispatch to `move_search_match(±1)` in `src/app.rs:9835-9842`. No new `Action` variants, no app-layer changes. No conflicts: text undo/redo uses Cmd/Ctrl+Z (`src/input/mod.rs:14-30`); Ctrl+N/P in the completion popup applies only to editor focus with the popup open.

2. **Status hint — `src/ui/mod.rs:2620-2623`**  
   `"{n} results  n/N next/prev  Enter locate  Esc close"` → `"{n} results  ↑/↓ select  Ctrl+N/P match  Enter locate  Esc close"`.

3. **Help palette — `src/help.rs:1889-1902`**  
   Move the `ExplorerSearchNext` / `ExplorerSearchPrevious` rows from `[ExplorerCatalogSearchConfirmed]` (unreachable) to `[ExplorerCatalogSearchEditing]`, key labels `Ctrl-n` / `Ctrl-p` (matching the existing literal `Ctrl-u` style at `src/help.rs:1843`). Literal sequences bypass the configurable-binding lookup (`src/help.rs:2849-2855`), so no config plumbing.

4. **Docs — `docs/keybindings.md`**  
   - Catalog-search paragraph (~line 203-208): describe Editing as accepting text, Backspace, Ctrl-U, `↑/↓` navigation, `Ctrl+N/P` match jump, Enter locate, Esc close; drop the false "after confirmation, `n`, `N`, and Esc are the active controls" claim for search (keep it for find).
   - Explorer tree table (~line 191): restrict the `n/N` row to find ("Next/previous confirmed find match").

## Testing

- **Keymap unit test** — extend `explorer_search_preempts_normal_bindings_and_edits_query` (`src/input/keymap.rs:3751`): assert Ctrl+N → `ExplorerSearchNext`, Ctrl+P → `ExplorerSearchPrevious`, and plain `n` / `N` → `ExplorerSearchInsert`.
- **Render test** — new test next to `explorer_find_shows_phase_specific_navigation_hint` (`tests/ui_render.rs:3851`): open catalog search, type a query, assert the buffer contains `↑/↓ select  Ctrl+N/P match` and does not contain `n/N next/prev`.
- Error behavior: none new — with zero match rows `move_search_match` is a no-op (`src/model/workspace.rs:789`).
