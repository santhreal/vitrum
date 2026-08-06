//! Unit tests for fast lookup-table ANSI color and attribute encoder.

use crate::color::{AnsiColor, Attribute, Style};

#[test]
fn test_basic_16_color_lookup() {
    let red_fg = Style::new().fg(AnsiColor::Basic(1));
    assert_eq!(red_fg.paint("error"), "\x1b[31merror\x1b[0m");

    let bright_cyan_bg = Style::new().bg(AnsiColor::Basic(14));
    assert_eq!(bright_cyan_bg.paint("highlight"), "\x1b[106mhighlight\x1b[0m");
}

#[test]
fn test_256_color_and_rgb() {
    let fixed = Style::new().fg(AnsiColor::Fixed(208));
    assert_eq!(fixed.paint("orange"), "\x1b[38;5;208morange\x1b[0m");

    let rgb = Style::new().fg(AnsiColor::Rgb(255, 100, 50));
    assert_eq!(rgb.paint("custom"), "\x1b[38;2;255;100;50mcustom\x1b[0m");
}

#[test]
fn test_attributes_and_styles() {
    let bold_italic_underlined = Style::new()
        .attr(Attribute::Bold)
        .attr(Attribute::Italic)
        .attr(Attribute::Underline)
        .fg(AnsiColor::Basic(2));

    let output = bold_italic_underlined.paint("styled");
    assert!(output.contains("\x1b[1m"));
    assert!(output.contains("\x1b[3m"));
    assert!(output.contains("\x1b[4m"));
    assert!(output.contains("\x1b[32m"));
    assert!(output.ends_with("\x1b[0m"));
}

#[test]
fn test_256_color_lookup_all_entries() {
    for idx in 0..=255 {
        let fg = Style::new().fg(AnsiColor::Fixed(idx));
        let expected_fg = format!("\x1b[38;5;{idx}mtest\x1b[0m");
        assert_eq!(fg.paint("test"), expected_fg);

        let bg = Style::new().bg(AnsiColor::Fixed(idx));
        let expected_bg = format!("\x1b[48;5;{idx}mtest\x1b[0m");
        assert_eq!(bg.paint("test"), expected_bg);
    }
}
