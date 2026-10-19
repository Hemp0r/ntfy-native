//! System UI fonts. eframe is built without its bundled fonts (their licence is not OSI).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontRole {
    Proportional,
    Monospace,
    Symbols,
    /// Chinese/Japanese fallback; optional, so missing files are not reported.
    Cjk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontOs {
    MacOs,
    Windows { windir: PathBuf },
    Other,
}

impl FontOs {
    pub fn current() -> FontOs {
        if cfg!(target_os = "macos") {
            FontOs::MacOs
        } else if cfg!(target_os = "windows") {
            let windir = std::env::var_os("WINDIR")
                .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
            FontOs::Windows { windir }
        } else {
            FontOs::Other
        }
    }
}

/// In order of preference; per role the first file that loads wins.
pub fn font_candidates(os: &FontOs) -> Vec<(FontRole, PathBuf)> {
    match os {
        FontOs::MacOs => {
            let dir = Path::new("/System/Library/Fonts");
            vec![
                (FontRole::Proportional, dir.join("SFNS.ttf")),
                (FontRole::Proportional, dir.join("Helvetica.ttc")),
                (FontRole::Monospace, dir.join("Menlo.ttc")),
                (FontRole::Symbols, dir.join("Apple Symbols.ttf")),
                (FontRole::Cjk, dir.join("PingFang.ttc")),
                (FontRole::Cjk, dir.join("Hiragino Sans GB.ttc")),
            ]
        }
        FontOs::Windows { windir } => {
            let dir = windir.join("Fonts");
            vec![
                (FontRole::Proportional, dir.join("segoeui.ttf")),
                (FontRole::Monospace, dir.join("consola.ttf")),
                (FontRole::Symbols, dir.join("seguisym.ttf")),
                (FontRole::Cjk, dir.join("msyh.ttc")),
                (FontRole::Cjk, dir.join("YuGothM.ttc")),
            ]
        }
        FontOs::Other => Vec::new(),
    }
}

/// The first readable candidate of each role, in candidate order.
pub fn load_fonts(
    candidates: &[(FontRole, PathBuf)],
    read: impl Fn(&Path) -> io::Result<Vec<u8>>,
) -> Vec<(FontRole, Vec<u8>)> {
    let mut loaded: Vec<(FontRole, Vec<u8>)> = Vec::new();
    for (role, path) in candidates {
        if loaded.iter().any(|(r, _)| r == role) {
            continue;
        }
        match read(path) {
            Ok(bytes) => loaded.push((*role, bytes)),
            Err(e) if *role == FontRole::Cjk => {
                tracing::debug!(path = %path.display(), error = %e, "CJK fallback font unavailable");
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "system font unavailable");
            }
        }
    }
    loaded
}

fn font_name(role: FontRole) -> &'static str {
    match role {
        FontRole::Proportional => "ui",
        FontRole::Monospace => "mono",
        FontRole::Symbols => "symbols",
        FontRole::Cjk => "cjk",
    }
}

/// Proportional: UI font, then symbols, then CJK. Monospace: mono font, then
/// the UI font, symbols and CJK.
pub fn definitions(fonts: Vec<(FontRole, Vec<u8>)>) -> FontDefinitions {
    let mut defs = FontDefinitions::empty();
    let has = |role| fonts.iter().any(|(r, _)| *r == role);
    let pick = |roles: &[FontRole]| -> Vec<String> {
        roles
            .iter()
            .filter(|r| has(**r))
            .map(|r| font_name(*r).to_owned())
            .collect()
    };
    let (ui, mono, symbols, cjk) = (
        FontRole::Proportional,
        FontRole::Monospace,
        FontRole::Symbols,
        FontRole::Cjk,
    );
    defs.families
        .insert(FontFamily::Proportional, pick(&[ui, symbols, cjk]));
    defs.families
        .insert(FontFamily::Monospace, pick(&[mono, ui, symbols, cjk]));
    for (role, bytes) in fonts {
        defs.font_data.insert(
            font_name(role).to_owned(),
            Arc::new(FontData::from_owned(bytes)),
        );
    }
    defs
}

/// Missing files are skipped; without any font egui draws boxes, but the window still works.
pub fn install(ctx: &egui::Context) {
    let loaded = load_fonts(&font_candidates(&FontOs::current()), |p| std::fs::read(p));
    if loaded.is_empty() {
        tracing::error!("no system font could be loaded, text will not be readable");
    } else if !loaded.iter().any(|(r, _)| *r == FontRole::Proportional) {
        tracing::warn!("no system UI font found, using fallbacks");
    }
    ctx.set_fonts(definitions(loaded));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(c: &[(FontRole, PathBuf)], role: FontRole) -> Vec<String> {
        c.iter()
            .filter(|(r, _)| *r == role)
            .map(|(_, p)| p.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn cjk_fallbacks() {
        let mac = font_candidates(&FontOs::MacOs);
        assert_eq!(
            paths(&mac, FontRole::Cjk),
            [
                "/System/Library/Fonts/PingFang.ttc",
                "/System/Library/Fonts/Hiragino Sans GB.ttc"
            ]
        );
        let win = font_candidates(&FontOs::Windows {
            windir: PathBuf::from("D:/Win"),
        });
        assert_eq!(
            paths(&win, FontRole::Cjk),
            ["D:/Win/Fonts/msyh.ttc", "D:/Win/Fonts/YuGothM.ttc"]
        );
        // A missing CJK font is no problem: the next one or none.
        let loaded = load_fonts(&mac, |p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            match name.as_str() {
                "PingFang.ttc" => Err(io::ErrorKind::NotFound.into()),
                _ => Ok(name.into_bytes()),
            }
        });
        assert!(loaded.contains(&(FontRole::Cjk, b"Hiragino Sans GB.ttc".to_vec())));
        let defs = definitions(vec![
            (FontRole::Proportional, vec![1]),
            (FontRole::Monospace, vec![2]),
            (FontRole::Symbols, vec![3]),
            (FontRole::Cjk, vec![4]),
        ]);
        assert_eq!(
            defs.families[&FontFamily::Proportional],
            ["ui", "symbols", "cjk"]
        );
        assert_eq!(
            defs.families[&FontFamily::Monospace],
            ["mono", "ui", "symbols", "cjk"]
        );
    }

    #[test]
    fn macos_candidates() {
        let c = font_candidates(&FontOs::MacOs);
        assert_eq!(
            paths(&c, FontRole::Proportional),
            [
                "/System/Library/Fonts/SFNS.ttf",
                "/System/Library/Fonts/Helvetica.ttc"
            ]
        );
        assert_eq!(
            paths(&c, FontRole::Monospace),
            ["/System/Library/Fonts/Menlo.ttc"]
        );
        assert_eq!(
            paths(&c, FontRole::Symbols),
            ["/System/Library/Fonts/Apple Symbols.ttf"]
        );
    }

    #[test]
    fn windows_candidates_use_windir() {
        let c = font_candidates(&FontOs::Windows {
            windir: PathBuf::from("D:/Win"),
        });
        assert_eq!(
            paths(&c, FontRole::Proportional),
            ["D:/Win/Fonts/segoeui.ttf"]
        );
        assert_eq!(paths(&c, FontRole::Monospace), ["D:/Win/Fonts/consola.ttf"]);
        assert_eq!(paths(&c, FontRole::Symbols), ["D:/Win/Fonts/seguisym.ttf"]);
        assert!(font_candidates(&FontOs::Other).is_empty());
    }

    #[test]
    fn first_loadable_candidate_per_role_wins() {
        let c = font_candidates(&FontOs::MacOs);
        let loaded = load_fonts(&c, |p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            match name.as_str() {
                "SFNS.ttf" | "Menlo.ttc" => Err(io::ErrorKind::NotFound.into()),
                _ => Ok(name.into_bytes()),
            }
        });
        assert_eq!(
            loaded,
            vec![
                (FontRole::Proportional, b"Helvetica.ttc".to_vec()),
                (FontRole::Symbols, b"Apple Symbols.ttf".to_vec()),
                (FontRole::Cjk, b"PingFang.ttc".to_vec()),
            ]
        );
        let all = load_fonts(&c, |p| Ok(p.to_string_lossy().into_owned().into_bytes()));
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].1, b"/System/Library/Fonts/SFNS.ttf");
        assert!(load_fonts(&c, |_| Err(io::ErrorKind::NotFound.into())).is_empty());
    }

    /// Skipped where the platform fonts are absent (e.g. Linux CI).
    #[test]
    fn system_fonts_cover_all_ui_strings() {
        let loaded = load_fonts(&font_candidates(&FontOs::current()), |p| std::fs::read(p));
        if !loaded.iter().any(|(r, _)| *r == FontRole::Proportional) {
            return;
        }
        let has_cjk = loaded.iter().any(|(r, _)| *r == FontRole::Cjk);
        let defs = definitions(loaded);
        let mut texts: Vec<&str> = crate::i18n::UI_DE
            .fields()
            .into_iter()
            .map(|f| f.1)
            .collect();
        texts.extend(crate::i18n::UI_EN.fields().into_iter().map(|f| f.1));
        texts.extend(["▾", "·", "…"]);
        if has_cjk {
            // Message titles and topics may be Chinese or Japanese.
            texts.extend(["备份完成", "バックアップ完了"]);
        }
        // The harness panics on any glyph no font can draw.
        let mut h = egui_kittest::Harness::builder()
            .with_context_setup(move |ctx| ctx.set_fonts(defs))
            .build_ui(move |ui| {
                for t in &texts {
                    ui.label(*t);
                    ui.monospace(*t);
                }
            });
        h.run();
    }

    #[test]
    fn families_get_fallbacks() {
        let defs = definitions(vec![
            (FontRole::Proportional, vec![1]),
            (FontRole::Monospace, vec![2]),
            (FontRole::Symbols, vec![3]),
        ]);
        assert_eq!(defs.font_data.len(), 3);
        assert_eq!(defs.families[&FontFamily::Proportional], ["ui", "symbols"]);
        assert_eq!(
            defs.families[&FontFamily::Monospace],
            ["mono", "ui", "symbols"]
        );
        let only_ui = definitions(vec![(FontRole::Proportional, vec![1])]);
        assert_eq!(only_ui.families[&FontFamily::Monospace], ["ui"]);
        let none = definitions(Vec::new());
        assert!(none.font_data.is_empty());
        assert!(none.families.values().all(Vec::is_empty));
    }
}
