use luck_config::{FormatConfig, parse_luck_config};
use luck_formatter::{FormatOptions, IndentStyle};

#[test]
fn format_config_fields_override_the_default_options() {
    let config = parse_luck_config(r#"{"format":{"indent_style":"spaces","indent_width":2}}"#)
        .expect("config parses");
    let options = FormatOptions::from(&config.format.expect("format section present"));
    assert!(matches!(options.indent_style, IndentStyle::Spaces));
    assert_eq!(options.indent_width, 2);
}

#[test]
fn empty_format_config_yields_the_default_options() {
    let options = FormatOptions::from(&FormatConfig::default());
    let defaults = FormatOptions::default();
    assert_eq!(options.indent_width, defaults.indent_width);
    assert_eq!(options.line_width, defaults.line_width);
}
