use cxx_qt_lib::{QList, QMap, QMapPair_QString_QVariant, QString, QVariant, QVariantValue};
use tako_config::TerminalConfig;

type InitialProperties = QMap<QMapPair_QString_QVariant>;
type VariantList = QList<QVariant>;

pub fn initial_properties() -> InitialProperties {
    let config = TerminalConfig::load_standard();
    let mut properties = InitialProperties::default();

    insert(
        &mut properties,
        "configFontFamily",
        &QString::from(config.font_family.as_deref().unwrap_or_default()),
    );
    insert(
        &mut properties,
        "configFontPointSize",
        &config.font_point_size.unwrap_or(13.5),
    );
    insert(
        &mut properties,
        "configForegroundColor",
        &color(config.foreground),
    );
    insert(
        &mut properties,
        "configBackgroundColor",
        &color(config.background),
    );
    insert(
        &mut properties,
        "configCursorColor",
        &color(config.cursor_color),
    );
    insert(
        &mut properties,
        "configColorPalette",
        &color_palette(config.color_palette.as_deref().unwrap_or_default()),
    );
    insert(
        &mut properties,
        "configCursorStyle",
        &config
            .cursor_style
            .map_or(1, tako_config::CursorStyle::terminal_view_value),
    );
    insert(
        &mut properties,
        "configCursorBlink",
        &config.cursor_blink.unwrap_or(false),
    );
    insert(
        &mut properties,
        "configScrollbackLimit",
        &(config.scrollback_limit.unwrap_or(10000) as u64),
    );

    properties
}

fn insert<T>(properties: &mut InitialProperties, name: &str, value: &T)
where
    T: QVariantValue,
{
    properties.insert_clone(&QString::from(name), &QVariant::from(value));
}

fn color(value: Option<tako_config::Rgb>) -> QString {
    value.map_or_else(QString::default, |rgb| QString::from(rgb.hex()))
}

fn color_palette(values: &[tako_config::Rgb]) -> VariantList {
    let mut list = VariantList::default();
    list.reserve(values.len() as isize);
    for rgb in values {
        list.append(QVariant::from(&QString::from(rgb.hex())));
    }
    list
}
