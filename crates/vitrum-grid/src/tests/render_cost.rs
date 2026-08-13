//! What a frame costs: upload counts, the zero-work idle path, and timing.
//!
//! The competitor failure this crate is designed against is a UI that re-uploads
//! or re-parses its whole state on a timer. These tests pin the exact number of
//! cells and buffer writes each kind of change produces, so a regression to
//! "just re-upload everything" fails a test instead of quietly costing a
//! milliseconds-per-frame tax on twenty sessions.

use std::time::Instant;

use crate::cell::{Attrs, Rgba, Style};
use crate::tests::support::{TEST_PX, draw, gpu, grid, renderer, renderer_with, target_for};

const FG: Rgba = Rgba::rgb(0xd0, 0xd0, 0xd0);
const BG: Rgba = Rgba::rgb(0x10, 0x10, 0x18);

/// Fill a grid with varied but reproducible content.
fn populate(g: &mut crate::CellGrid, seed: u32) {
    let style = Style::new(FG, BG);
    for row in 0..g.rows() {
        for col in 0..g.cols() {
            let n = seed
                .wrapping_add(u32::from(row) * 131)
                .wrapping_add(u32::from(col) * 17);
            let ch = char::from(b'!' + (n % 90) as u8);
            let attrs = match n % 4 {
                0 => Attrs::NONE,
                1 => Attrs::BOLD,
                2 => Attrs::ITALIC,
                _ => Attrs::UNDERLINE,
            };
            g.write_char(col, row, ch, style.with_attrs(attrs)).unwrap();
        }
    }
}

/// `count` distinct single-column characters starting the scan at `base`.
///
/// Combining marks, unassigned codepoints, and double-width blocks are skipped
/// so every character written from this pool occupies exactly one column.
fn narrow_chars(base: u32, count: usize) -> Vec<char> {
    let mut out = Vec::with_capacity(count);
    let mut cp = base;
    while out.len() < count {
        assert!(cp < 0x2_0000, "ran out of narrow codepoints below U+20000");
        if let Some(ch) = char::from_u32(cp)
            && crate::char_width(ch) == crate::CharWidth::Narrow
        {
            out.push(ch);
        }
        cp += 1;
    }
    out
}

/// The first frame must be a full rebuild that uploads every cell in one write.
///
/// Adjacent damage spans are coalesced so a full-screen change costs one
/// `write_buffer`, not one per row. Fifty separate writes per frame is the kind
/// of overhead that only shows up at twenty sessions, which is exactly when it
/// is hardest to diagnose.
#[test]
fn the_first_frame_uploads_every_cell_in_a_single_coalesced_write() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 40, 12);
    let mut g = grid(40, 12, Style::new(FG, BG));
    populate(&mut g, 1);

    let stats = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut g,
            target.view(),
            (target.width(), target.height()),
        )
        .unwrap();

    assert!(stats.full_rebuild, "the first frame has nothing to reuse");
    assert!(stats.gpu_work);
    assert_eq!(stats.cells_uploaded, 480, "40 x 12 cells");
    assert_eq!(stats.writes, 1, "the whole grid is one contiguous run");
    assert_eq!(stats.instances_drawn, 480);
    assert!(
        stats.glyphs_added > 0,
        "the first frame must populate the atlas"
    );
    assert_eq!(g.dirty_cells(), 0, "the render must consume the damage");
}

/// An unchanged frame must record no GPU work at all.
///
/// This is the whole idle story. No encoder is created, no buffer is written,
/// nothing is submitted, and no draw is issued. Twenty idle sessions cost
/// twenty of these. A regression here reintroduces the continuous-repaint cost
/// that makes a competitor burn CPU on a static screen.
#[test]
fn an_unchanged_frame_records_no_gpu_work() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 40, 12);
    let mut g = grid(40, 12, Style::new(FG, BG));
    populate(&mut g, 2);
    draw(&mut renderer, &mut g, &target);

    for attempt in 0..5 {
        let stats = renderer
            .render(
                gpu().device(),
                gpu().queue(),
                &mut g,
                target.view(),
                (target.width(), target.height()),
            )
            .unwrap();
        assert!(
            !stats.gpu_work,
            "idle frame {attempt} recorded GPU work: {stats:?}"
        );
        assert_eq!(stats.cells_uploaded, 0);
        assert_eq!(stats.writes, 0);
        assert_eq!(stats.instances_drawn, 0);
        assert_eq!(stats.glyphs_added, 0);
        assert!(!stats.full_rebuild);
        assert_eq!(g.dirty_cells(), 0);
    }
}

/// Rewriting identical content must not resurrect damage.
///
/// A VT front end repainting a static status line writes the same bytes every
/// tick. If that counted as damage, "idle" would never happen in practice and
/// the zero-work path above would be dead code.
#[test]
fn repainting_identical_content_stays_on_the_zero_work_path() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 20, 4);
    let mut g = grid(20, 4, Style::new(FG, BG));
    populate(&mut g, 3);
    draw(&mut renderer, &mut g, &target);

    populate(&mut g, 3);
    let stats = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut g,
            target.view(),
            (target.width(), target.height()),
        )
        .unwrap();
    assert!(!stats.gpu_work, "an identical repaint must cost nothing");
}

/// An unchanged frame must leave the target's pixels exactly as they were.
///
/// The skip path is only safe if the previous frame is still there. If skipping
/// left a stale or cleared target, the terminal would flicker to black whenever
/// nothing was happening, which is the most visible bug imaginable.
#[test]
fn an_unchanged_frame_leaves_the_target_pixels_untouched() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 8, 3);
    let style = Style::new(FG, BG);
    let mut g = grid(8, 3, style);
    g.write_str(0, 0, "stable", style).unwrap();

    let first = draw(&mut renderer, &mut g, &target);
    let second = draw(&mut renderer, &mut g, &target);
    assert_eq!(
        first.as_bytes(),
        second.as_bytes(),
        "a skipped frame must preserve the previous image byte for byte"
    );
    let total = (first.width() * first.height()) as usize;
    assert!(
        first.count(BG) < total,
        "the reference image must actually contain text, not just background"
    );
}

/// A one-cell change must upload exactly one cell in exactly one write.
///
/// A keystroke is one cell. Uploading the row, or the screen, turns typing into
/// a bandwidth problem: at 200 columns that would be a 200-fold overhead per
/// character.
#[test]
fn a_single_cell_change_uploads_exactly_one_cell() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 40, 12);
    let style = Style::new(FG, BG);
    let mut g = grid(40, 12, style);
    populate(&mut g, 4);
    draw(&mut renderer, &mut g, &target);

    g.write_char(7, 5, 'Z', style).unwrap();
    let stats = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut g,
            target.view(),
            (target.width(), target.height()),
        )
        .unwrap();

    assert!(stats.gpu_work);
    assert!(!stats.full_rebuild, "one cell must not trigger a rebuild");
    assert_eq!(stats.cells_uploaded, 1);
    assert_eq!(stats.writes, 1);
    assert_eq!(
        stats.instances_drawn, 480,
        "all cells are drawn even though one was uploaded"
    );
}

/// Changes on two separated rows must produce two writes, not one big one.
///
/// Coalescing must only merge runs that are genuinely adjacent in the flat cell
/// order. A coalescer that merged across a gap would upload every cell between
/// the two edits, which for a change at the top and bottom of the screen is the
/// entire grid.
#[test]
fn changes_on_separated_rows_upload_separately() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 40, 12);
    let style = Style::new(FG, BG);
    let mut g = grid(40, 12, style);
    populate(&mut g, 5);
    draw(&mut renderer, &mut g, &target);

    g.write_char(3, 1, 'A', style).unwrap();
    g.write_char(9, 8, 'B', style).unwrap();
    let stats = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut g,
            target.view(),
            (target.width(), target.height()),
        )
        .unwrap();

    assert_eq!(stats.cells_uploaded, 2);
    assert_eq!(stats.writes, 2, "two distant edits are two runs");
}

/// Full-width damage on consecutive rows must coalesce into one write.
///
/// A scroll damages a whole block of rows. Those rows are contiguous in the
/// flat cell array, so uploading them as one run is both correct and much
/// cheaper than one write per row.
#[test]
fn full_width_damage_on_consecutive_rows_coalesces() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 40, 12);
    let style = Style::new(FG, BG);
    let mut g = grid(40, 12, style);
    populate(&mut g, 6);
    draw(&mut renderer, &mut g, &target);

    g.scroll_up(2, 6, 1, crate::Cell::blank(style)).unwrap();
    let stats = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut g,
            target.view(),
            (target.width(), target.height()),
        )
        .unwrap();

    assert_eq!(stats.cells_uploaded, 200, "five rows of forty");
    assert_eq!(stats.writes, 1, "five adjacent full rows are one run");
}

/// A resize must force a full rebuild and redraw at the new size.
///
/// The instance buffer is reallocated, so every instance in it is
/// uninitialised. Anything less than a full rebuild draws garbage for the cells
/// that were not re-uploaded.
#[test]
fn a_resize_forces_a_full_rebuild() {
    let mut renderer = renderer();
    let style = Style::new(FG, BG);
    let small = target_for(&renderer, 10, 4);
    let mut g = grid(10, 4, style);
    populate(&mut g, 7);
    draw(&mut renderer, &mut g, &small);

    g.resize(16, 6).unwrap();
    let large = target_for(&renderer, 16, 6);
    let stats = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut g,
            large.view(),
            (large.width(), large.height()),
        )
        .unwrap();

    assert!(stats.full_rebuild);
    assert_eq!(stats.cells_uploaded, 96);
    assert_eq!(stats.instances_drawn, 96);
    assert_eq!(stats.writes, 1);

    // And the frame after the resize is free again.
    let idle = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut g,
            large.view(),
            (large.width(), large.height()),
        )
        .unwrap();
    assert!(!idle.gpu_work);
}

/// Changing the viewport without changing the grid must still redraw.
///
/// The viewport feeds the vertex shader's pixel-to-clip conversion through a
/// uniform. Skipping the frame because the grid looked clean would leave the
/// old uniform in place and the grid would be drawn at the wrong scale until
/// something else changed.
#[test]
fn a_viewport_change_alone_forces_a_redraw() {
    let mut renderer = renderer();
    let style = Style::new(FG, BG);
    let mut g = grid(6, 2, style);
    populate(&mut g, 8);

    let exact = target_for(&renderer, 6, 2);
    draw(&mut renderer, &mut g, &exact);

    let padded = crate::HeadlessTarget::new(gpu().device(), exact.width() + 7, exact.height() + 5);
    let stats = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut g,
            padded.view(),
            (padded.width(), padded.height()),
        )
        .unwrap();
    assert!(stats.full_rebuild, "a new viewport invalidates the uniform");
    assert!(stats.gpu_work);
    assert_eq!(stats.cells_uploaded, 12);
}

/// `invalidate` must force the next frame to redraw.
///
/// A host that reallocated its swapchain, or drew something else over the
/// texture, has to be able to tell the renderer the previous frame is gone.
/// Without this the renderer would keep skipping and the terminal would stay
/// blank.
#[test]
fn invalidate_forces_the_next_frame_to_redraw() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 6, 2);
    let mut g = grid(6, 2, Style::new(FG, BG));
    populate(&mut g, 9);
    draw(&mut renderer, &mut g, &target);

    let idle = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut g,
            target.view(),
            (target.width(), target.height()),
        )
        .unwrap();
    assert!(!idle.gpu_work);

    renderer.invalidate();
    let forced = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut g,
            target.view(),
            (target.width(), target.height()),
        )
        .unwrap();
    assert!(forced.full_rebuild);
    assert_eq!(forced.cells_uploaded, 12);
}

/// A glyph seen again must not be rasterised or uploaded again.
///
/// Rasterising on every frame is the CPU cost this atlas exists to remove. The
/// counter is exact: a screen of repeated characters must add one atlas entry
/// per distinct character and no more.
#[test]
fn a_repeated_glyph_is_rasterized_only_once() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 20, 5);
    let style = Style::new(FG, BG);
    let mut g = grid(20, 5, style);
    for row in 0..5u16 {
        for col in 0..20u16 {
            g.write_char(col, row, 'w', style).unwrap();
        }
    }

    let first = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut g,
            target.view(),
            (target.width(), target.height()),
        )
        .unwrap();
    assert_eq!(
        first.glyphs_added, 1,
        "one hundred 'w' cells share one atlas entry"
    );

    // Same character in a new place: still no new atlas entry.
    g.write_char(0, 0, ' ', style).unwrap();
    g.write_char(0, 0, 'w', style).unwrap();
    g.write_char(19, 4, 'x', style).unwrap();
    let second = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut g,
            target.view(),
            (target.width(), target.height()),
        )
        .unwrap();
    assert_eq!(second.glyphs_added, 1, "only 'x' is new");
}

/// A mid-frame atlas reset must be repaired inside the same frame.
///
/// When the atlas fills, every coordinate already written this frame becomes
/// meaningless. The renderer detects the generation bump and rebuilds the whole
/// instance buffer before drawing. If it did not, the screen would show a
/// scramble of the wrong glyphs until some unrelated change forced a repaint.
#[test]
fn a_mid_frame_atlas_reset_is_repaired_before_drawing() {
    // A 256px atlas holds a few hundred small glyph boxes, so two screens of
    // distinct characters overflow it.
    let mut renderer = renderer_with(TEST_PX, 256);
    let target = target_for(&renderer, 16, 8);
    let style = Style::new(FG, BG);
    let mut g = grid(16, 8, style);

    // Distinct single-column characters. Scanning past combining marks and
    // double-width blocks keeps every cell one column wide, so the grid stays
    // a simple 16x8 of unique glyphs.
    let pool = narrow_chars(0x0100, 16 * 8 * 12);
    let fill = |g: &mut crate::CellGrid, base: usize| {
        for row in 0..g.rows() {
            for col in 0..g.cols() {
                let n = base + usize::from(row) * usize::from(g.cols()) + usize::from(col);
                g.write_char(col, row, pool[n], style).unwrap();
            }
        }
    };

    fill(&mut g, 0);
    let first = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut g,
            target.view(),
            (target.width(), target.height()),
        )
        .unwrap();
    assert_eq!(renderer.atlas().generation(), 0, "the first screen must fit");
    assert_eq!(first.cells_uploaded, 128);

    // A second, entirely different screen. Together they exceed the atlas.
    let mut generation = renderer.atlas().generation();
    let mut resets = 0;
    for round in 1..12u32 {
        fill(&mut g, round as usize * 128);
        renderer
            .render(
                gpu().device(),
                gpu().queue(),
                &mut g,
                target.view(),
                (target.width(), target.height()),
            )
            .expect("one reset per frame must keep every frame renderable");
        if renderer.atlas().generation() != generation {
            resets += 1;
            generation = renderer.atlas().generation();
        }
    }
    assert!(
        resets >= 1,
        "eleven screens of distinct glyphs must overflow a 256px atlas at least once"
    );

    // The frame after a reset must be free again: the reset was fully absorbed.
    let idle = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut g,
            target.view(),
            (target.width(), target.height()),
        )
        .unwrap();
    assert!(
        !idle.gpu_work,
        "an absorbed atlas reset must not leave the renderer permanently dirty: {idle:?}"
    );

    // And the pixels must be right, not stale coordinates from before the reset.
    let image = target.read(gpu().device(), gpu().queue());
    assert!(
        image.count(BG) < (image.width() * image.height()) as usize,
        "the screen must show glyphs, not just background"
    );
}

/// Frame timing for a full 200x50 redraw and for a no-change frame.
///
/// This is a reported measurement, not a threshold test, with one hard
/// assertion: the no-change path must record zero GPU work and zero uploads.
/// The timing is printed so a regression in the redraw path is visible in the
/// test log; the assertion is on the property that actually matters, because a
/// wall-clock threshold on shared CI hardware is a flaky test rather than a
/// useful one.
#[test]
fn frame_timing_for_a_full_redraw_and_a_no_change_frame() {
    const COLS: u16 = 200;
    const ROWS: u16 = 50;
    const CELLS: u32 = COLS as u32 * ROWS as u32;
    const ITERATIONS: u32 = 60;

    let mut renderer = renderer();
    let target = target_for(&renderer, COLS, ROWS);
    let style = Style::new(FG, BG);
    let mut g = grid(COLS, ROWS, style);
    populate(&mut g, 11);

    let device = gpu().device();
    let queue = gpu().queue();
    let viewport = (target.width(), target.height());
    let mut render = |g: &mut crate::CellGrid| {
        renderer
            .render(device, queue, g, target.view(), viewport)
            .expect("render must succeed")
    };

    // Warm up: build the atlas, compile the pipeline, allocate the buffer.
    for _ in 0..3 {
        g.mark_all_damaged();
        render(&mut g);
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device must not be lost");

    let mut full_cpu = Vec::with_capacity(ITERATIONS as usize);
    for _ in 0..ITERATIONS {
        g.mark_all_damaged();
        let start = Instant::now();
        let stats = render(&mut g);
        let cpu = start.elapsed();
        assert!(stats.gpu_work);
        assert_eq!(stats.cells_uploaded, CELLS);
        assert_eq!(stats.writes, 1);
        assert_eq!(stats.instances_drawn, CELLS);
        full_cpu.push(cpu);
    }
    let start = Instant::now();
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device must not be lost");
    let drain = start.elapsed();

    let mut idle_cpu = Vec::with_capacity(ITERATIONS as usize);
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let stats = render(&mut g);
        idle_cpu.push(start.elapsed());
        assert!(
            !stats.gpu_work,
            "the no-change path must record zero GPU work"
        );
        assert_eq!(stats.cells_uploaded, 0);
        assert_eq!(stats.writes, 0);
        assert_eq!(stats.instances_drawn, 0);
        assert_eq!(stats.glyphs_added, 0);
    }

    // One cell changes: a cursor blink, or a character arriving. This is the
    // case a terminal is in almost all the time, and it is the one that
    // distinguishes an O(damage) renderer from an O(visible) one, so it is
    // reported alongside the two extremes rather than inferred from them.
    let mut one_cell_cpu = Vec::with_capacity(ITERATIONS as usize);
    for i in 0..ITERATIONS {
        let col = (i % u32::from(COLS)) as u16;
        g.write_char(col, 0, 'x', style)
            .expect("writing one cell must succeed");
        let start = Instant::now();
        let stats = render(&mut g);
        one_cell_cpu.push(start.elapsed());
        assert!(stats.gpu_work);
        assert_eq!(
            stats.cells_uploaded, 1,
            "one changed cell must upload exactly one cell"
        );
        assert_eq!(stats.writes, 1);
    }

    let mean = |v: &[std::time::Duration]| {
        v.iter().sum::<std::time::Duration>().as_secs_f64() * 1e6 / v.len() as f64
    };
    let median = |v: &mut Vec<std::time::Duration>| {
        v.sort_unstable();
        v[v.len() / 2].as_secs_f64() * 1e6
    };
    let full_mean = mean(&full_cpu);
    let idle_mean = mean(&idle_cpu);
    let one_cell_mean = mean(&one_cell_cpu);
    let full_median = median(&mut full_cpu);
    let idle_median = median(&mut idle_cpu);
    let one_cell_median = median(&mut one_cell_cpu);

    println!("--- vitrum-grid frame timing ({COLS}x{ROWS} = {CELLS} cells) ---");
    println!("adapter:              {}", gpu().describe());
    println!("font:                 {} at {TEST_PX}px", renderer.fonts().family());
    println!("cell size:            {:?} px", renderer.cell_size());
    println!("full redraw (CPU):    mean {full_mean:.1} us, median {full_median:.1} us");
    println!("one-cell change (CPU):mean {one_cell_mean:.3} us, median {one_cell_median:.3} us");
    println!("no-change frame (CPU):mean {idle_mean:.3} us, median {idle_median:.3} us");
    println!(
        "gpu drain for {ITERATIONS} queued full redraws: {:.1} us total, {:.1} us each",
        drain.as_secs_f64() * 1e6,
        drain.as_secs_f64() * 1e6 / f64::from(ITERATIONS)
    );
    println!("no-change frame GPU work: none (no encoder, no write_buffer, no submit)");

    assert!(
        idle_mean < full_mean,
        "the no-change path must be cheaper than a full redraw: {idle_mean:.3} us vs {full_mean:.1} us"
    );
}
