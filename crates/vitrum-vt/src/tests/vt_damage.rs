//! What does and does not cost work.
//!
//! The whole reason twenty idle sessions are affordable is that an idle session
//! produces no damage, and therefore no GPU upload. These tests assert on the
//! counts rather than on the screen, because the screen looks the same whether
//! the frame cost nothing or redrew everything.

use super::support::Fixture;

#[test]
fn a_sync_with_no_new_bytes_changes_nothing() {
    let mut fx = Fixture::new(20, 5);
    fx.write(b"hello");

    let idle = fx.sync();
    assert!(idle.is_noop(), "idle sync did work: {idle:?}");
    assert_eq!(idle.cells_changed, 0);
}

#[test]
fn an_unchanged_terminal_reads_no_rows_at_all() {
    // Not reading the rows is what makes the idle path free: a clean frame must
    // not walk fifty rows to discover fifty rows are clean.
    let mut fx = Fixture::new(20, 5);
    fx.write(b"hello");

    let idle = fx.sync();
    assert_eq!(idle.rows_synced, 0);
    assert_eq!(idle.rows_skipped, 5);
}

#[test]
fn writing_one_row_does_not_redraw_the_others() {
    let mut fx = Fixture::new(20, 5);
    fx.write(b"one\r\ntwo\r\nthree");

    let stats = fx.write(b"\x1b[1;1Hx");
    // Two rows are read, not one: the row written, and the row the cursor left,
    // which has to repaint without the cursor on it. Three of the five rows are
    // still never touched, which is the property that matters.
    assert_eq!(stats.rows_synced, 2);
    assert_eq!(stats.rows_skipped, 3);
    assert_eq!(stats.cells_changed, 1, "only the touched cell changes");
}

#[test]
fn rewriting_the_same_text_costs_no_cells() {
    // The terminal reports the row dirty because bytes arrived, but the values
    // are identical, so the grid must record no damage. This is the case that
    // makes a repainting full-screen program cheap.
    let mut fx = Fixture::new(20, 3);
    fx.write(b"\x1b[1;1Hsame text");

    let stats = fx.write(b"\x1b[1;1Hsame text");
    assert!(stats.rows_synced > 0, "the row was read");
    assert_eq!(stats.cells_changed, 0, "but nothing differed");
    assert!(stats.is_noop());
}

#[test]
fn a_colour_change_alone_is_damage() {
    // Same characters, different colour. A projection that compared only
    // characters would leave the old colour on screen.
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x1b[1;1Habc");

    let stats = fx.write(b"\x1b[1;1H\x1b[31mabc");
    assert_eq!(stats.cells_changed, 3);
}

#[test]
fn the_first_sync_of_a_screen_is_not_a_noop() {
    let mut fx = Fixture::new(10, 2);
    let stats = fx.write(b"x");
    assert!(!stats.is_noop());
    assert!(stats.cells_changed > 0);
}

#[test]
fn a_resize_redraws_everything_even_when_the_terminal_is_clean() {
    // The terminal can consider every row unchanged while the grid has never
    // seen the new geometry. Trusting the dirty flags here would show a screen
    // full of blanks.
    let mut fx = Fixture::new(20, 5);
    fx.write(b"hello");
    assert!(fx.sync().is_noop());

    fx.vt.resize(30, 6, (8, 16)).expect("resize succeeds");
    let stats = fx.sync();

    assert!(stats.resized);
    assert!(!stats.is_noop());
    assert_eq!(fx.grid.cols(), 30);
    assert_eq!(fx.grid.rows(), 6);
    assert_eq!(fx.line(0), "hello");
}

#[test]
fn the_grid_is_resized_to_the_terminal_even_when_it_starts_wrong() {
    // A caller may hand in any grid. Correctness must not depend on the caller
    // having sized it, because the terminal is the authority on its own size.
    let mut fx = Fixture::new(20, 5);
    fx.grid = vitrum_grid::CellGrid::new(4, 2, vitrum_grid::Style::DEFAULT).expect("grid");

    let stats = fx.write(b"hello");
    assert!(stats.resized);
    assert_eq!(fx.grid.cols(), 20);
    assert_eq!(fx.line(0), "hello");
}

#[test]
fn damage_survives_until_the_renderer_clears_it() {
    // The grid's damage belongs to the renderer, not to the sync. A sync that
    // cleared it would drop a frame whenever two syncs happened between paints.
    let mut fx = Fixture::new(10, 2);
    fx.write(b"x");
    assert!(fx.grid.is_dirty());

    fx.sync();
    assert!(fx.grid.is_dirty(), "a second sync must not clear damage");

    fx.grid.clear_damage();
    assert!(!fx.grid.is_dirty());
}
