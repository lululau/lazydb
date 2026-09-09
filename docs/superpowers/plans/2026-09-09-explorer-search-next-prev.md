# Explorer Catalog Search Ctrl+N/P Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the `f` catalog search real match-to-match navigation (Ctrl+N/P) and make its status hint, help palette, and docs name only keys that actually work.

**Architecture:** The catalog search never leaves `ExplorerSearchPhase::Editing` (Enter locates and closes; the Confirmed branch is unreachable), so match navigation is added to the Editing keymap branch using the existing `Action::ExplorerSearchNext/Previous` actions — no new Action variants, no app-layer changes. UI hint, help palette rows, and docs are updated to match. Spec: `docs/superpowers/specs/2026-09-09-explorer-search-next-prev-design.md`.

**Tech Stack:** Rust (ratatui/crossterm TUI), cargo test.

---

### Task 1: Keymap — Ctrl+N/P in catalog search Editing branch

**Files:**
- Modify: `src/input/keymap.rs:644-649` (Editing branch of the catalog-search handler)
- Test: `src/input/keymap.rs:3751-3813` (`explorer_search_preempts_normal_bindings_and_edits_query`)

- [ ] **Step 1: Write the failing test**

In `src/input/keymap.rs`, inside `explorer_search_preempts_normal_bindings_and_edits_query`, insert these assertions after the Down/Home/End block (after the `assert_eq!` matching `Some(Action::ExplorerSearchMove(isize::MAX))` at ~line 3783-3786):

```rust
        assert_eq!(
            keymap.map(control_key('n'), &app),
            Some(Action::ExplorerSearchNext)
        );
        assert_eq!(
            keymap.map(control_key('p'), &app),
            Some(Action::ExplorerSearchPrevious)
        );
        assert_eq!(
            keymap.map(key(KeyCode::Char('n')), &app),
            Some(Action::ExplorerSearchInsert('n'))
        );
        assert_eq!(
            keymap.map(
                KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT),
                &app
            ),
            Some(Action::ExplorerSearchInsert('N'))
        );
```

(`control_key` helper already exists at `src/input/keymap.rs:3331`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib input::keymap::tests::explorer_search_preempts_normal_bindings_and_edits_query`
Expected: FAIL — the first two asserts get `None` (Ctrl+N/P currently fall through the modifier guard at line 647-649).

- [ ] **Step 3: Write minimal implementation**

In `src/input/keymap.rs`, in the catalog-search Editing branch (the `else` starting ~line 636), after the Ctrl+U block:

```rust
                if event.modifiers == KeyModifiers::CONTROL && event.code == KeyCode::Char('u') {
                    return Some(Action::ExplorerSearchClear);
                }
```

insert:

```rust
                if event.modifiers == KeyModifiers::CONTROL && event.code == KeyCode::Char('n') {
                    return Some(Action::ExplorerSearchNext);
                }
                if event.modifiers == KeyModifiers::CONTROL && event.code == KeyCode::Char('p') {
                    return Some(Action::ExplorerSearchPrevious);
                }
```

Style note: flat single-condition `if`s exactly match the Ctrl+U line above and the completion-popup precedent at `src/input/keymap.rs:837-843`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib input::keymap::tests::explorer_search_preempts_normal_bindings_and_edits_query`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/input/keymap.rs
git commit -m "feat(explorer): Ctrl+N/P jump between catalog search matches"
```

---

### Task 2: Status hint names real keys

**Files:**
- Modify: `src/ui/mod.rs:2620-2623` (`render_explorer_search` status line)
- Test: `tests/ui_render.rs` (new test next to the find-hint test at line 3851)

- [ ] **Step 1: Write the failing test**

In `tests/ui_render.rs`, add after `explorer_find_shows_phase_specific_navigation_hint` (ends ~line 3866):

```rust
#[test]
fn explorer_search_status_hint_names_editing_controls() {
    let mut app = fixture();
    app.focus = Focus::Explorer;
    app.update(Action::ExplorerSearchOpen);
    app.update(Action::ExplorerSearchInsert('u'));
    let output = render(&app, 180, 36);

    assert!(output.contains("0 results  ↑/↓ select  Ctrl+N/P match"), "{output}");
    assert!(!output.contains("n/N next/prev"), "{output}");
}
```

Rationale for the fixture: `fixture()` has no catalog rows, so typing `u` makes `refresh_frontend_search` set `Ready` with zero matches and the status line renders `0 results  …` (`src/ui/mod.rs:2620-2623`). Width 180 keeps the explorer pane wide enough (56 cols, adaptive clamp) that the asserted prefix is not clipped.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test ui_render explorer_search_status_hint_names_editing_controls`
Expected: FAIL — output contains `n/N next/prev`, not `↑/↓ select  Ctrl+N/P match`.

- [ ] **Step 3: Write minimal implementation**

In `src/ui/mod.rs`, `render_explorer_search`, change:

```rust
            _ => format!(
                "{} results  n/N next/prev  Enter locate  Esc close",
                search.frontend_match_rows.len()
            ),
```

to:

```rust
            _ => format!(
                "{} results  ↑/↓ select  Ctrl+N/P match  Enter locate  Esc close",
                search.frontend_match_rows.len()
            ),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test ui_render explorer_search_status_hint_names_editing_controls`
Expected: PASS

Also run the neighboring search tests to confirm no regression:
Run: `cargo test --test ui_render explorer_search`
Expected: PASS (all)

- [ ] **Step 5: Commit**

```bash
git add src/ui/mod.rs tests/ui_render.rs
git commit -m "fix(ui): catalog search hint lists keys that actually work"
```

---

### Task 3: Help palette rows move to the reachable Editing context

**Files:**
- Modify: `src/help.rs:1875-1902` (SHORTCUT_CATALOG rows) and `src/help.rs:4305-4312` (test expectations)
- Test: `src/help.rs:4296` (`footer_modal_contexts_have_actual_controls`)

- [ ] **Step 1: Update the test to the expected end state (fails first)**

In `src/help.rs`, in `footer_modal_contexts_have_actual_controls`, replace:

```rust
            (
                ShortcutContext::ExplorerCatalogSearchEditing,
                vec!["type / Backspace", "Enter"],
            ),
            (
                ShortcutContext::ExplorerCatalogSearchConfirmed,
                vec!["n", "N"],
            ),
```

with:

```rust
            (
                ShortcutContext::ExplorerCatalogSearchEditing,
                vec!["type / Backspace", "Ctrl-n", "Ctrl-p", "Enter"],
            ),
```

(The `ExplorerCatalogSearchConfirmed` case is removed: after the row move that context has no controls, and this test asserts contexts list their actual controls. The context variant itself stays — see spec non-goals.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib help::tests::footer_modal_contexts_have_actual_controls`
Expected: FAIL — Editing footer is still `["type / Backspace", "Enter"]`.

- [ ] **Step 3: Write minimal implementation**

In `src/help.rs` SHORTCUT_CATALOG, replace the four catalog-search rows:

```rust
    row!(
        ExplorerSearchEdit,
        [ExplorerCatalogSearchEditing],
        "type / Backspace",
        "edit catalog search",
        display
    ),
    row!(
        ExplorerSearchLocate,
        [ExplorerCatalogSearchEditing],
        "Enter",
        "locate selected result",
        display
    ),
    row!(
        ExplorerSearchNext,
        [ExplorerCatalogSearchConfirmed],
        "n",
        "next match",
        display
    ),
    row!(
        ExplorerSearchPrevious,
        [ExplorerCatalogSearchConfirmed],
        "N",
        "previous match",
        display
    ),
```

with:

```rust
    row!(
        ExplorerSearchEdit,
        [ExplorerCatalogSearchEditing],
        "type / Backspace",
        "edit catalog search",
        display
    ),
    row!(
        ExplorerSearchNext,
        [ExplorerCatalogSearchEditing],
        "Ctrl-n",
        "next match",
        display
    ),
    row!(
        ExplorerSearchPrevious,
        [ExplorerCatalogSearchEditing],
        "Ctrl-p",
        "previous match",
        display
    ),
    row!(
        ExplorerSearchLocate,
        [ExplorerCatalogSearchEditing],
        "Enter",
        "locate selected result",
        display
    ),
```

Notes: the `Ctrl-n` label matches the literal `Ctrl-u` style at `src/help.rs:1843`; literal sequences bypass the configurable-binding lookup (`src/help.rs:2849-2855`). Rows are reordered so the footer reads type → navigate → locate. `display` flag and the movement-direction weights at `src/help.rs:536/568` (`ExplorerSearchNext` = 1, `ExplorerSearchPrevious` = -1) are untouched.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib help::`
Expected: PASS (all help tests, including `shortcut_context_resolves_...` which sets `phase = Confirmed` directly — the variant still exists)

- [ ] **Step 5: Commit**

```bash
git add src/help.rs
git commit -m "fix(help): catalog search next/prev rows under the Editing context"
```

---

### Task 4: Docs

**Files:**
- Modify: `docs/keybindings.md:191` and `docs/keybindings.md:203-208`

- [ ] **Step 1: Update the explorer tree table row**

In `docs/keybindings.md`, change line 191:

```markdown
| `n/N` | Next/previous confirmed find/search match |
```

to:

```markdown
| `n/N` | Next/previous confirmed find match |
```

(`n/N` in the tree only ever applies to `/` find; catalog search never confirms.)

- [ ] **Step 2: Update the search-mode paragraph**

Change:

```markdown
Visible-node find is Editing until Enter. While Editing, printable keys,
Backspace, Ctrl-U, Enter, and Esc belong to find input. After confirmation,
`n`, `N`, and Esc cycle or close it. Catalog search Editing accepts text,
Backspace, Ctrl-U, navigation, Enter to locate, and Esc; after confirmation,
`n`, `N`, and Esc are the active controls.
```

to:

```markdown
Visible-node find is Editing until Enter. While Editing, printable keys,
Backspace, Ctrl-U, Enter, and Esc belong to find input. After confirmation,
`n`, `N`, and Esc cycle or close it. Catalog search Editing accepts text,
Backspace, Ctrl-U, arrow-key selection, Ctrl-N/Ctrl-P to jump between
matches, Enter to locate, and Esc.
```

- [ ] **Step 3: Commit**

```bash
git add docs/keybindings.md
git commit -m "docs: catalog search match jump uses Ctrl-N/Ctrl-P"
```

---

### Task 5: Full verification

- [ ] **Step 1: Format**

Run: `cargo fmt`
Expected: no output; if it rewrites files, `git diff` to confirm only this feature's files changed and `git commit -m "style: format explorer search match jump"`.

- [ ] **Step 2: Run the full test suite**

Run: `cargo test`
Expected: PASS (no failures)

- [ ] **Step 3: Manual smoke test (optional but recommended)**

Run: `cargo run`, connect to any profile, press `f`, type a query, then:
- Ctrl+N / Ctrl+P jump between highlighted matches (wraps around)
- ↑/↓ move row by row
- plain `n` / `N` still type into the query
- Enter locates and closes; the bottom hint reads `↑/↓ select  Ctrl+N/P match  Enter locate  Esc close`

If anything fails, fix before proceeding; do not mark the plan complete with failing tests.
