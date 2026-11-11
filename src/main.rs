mod data;
mod schema;
mod ui;

use std::path::Path;
use std::rc::Rc;

use gpui::*;
use gpui_platform::application;

use crate::data::LazyParquetSource;
use crate::ui::{AppState, RootView};

fn main() {
    let state = match std::env::args().nth(1) {
        None => AppState::Error("Usage: parqueter <file.parquet>".to_string()),
        Some(path_str) => {
            let path = Path::new(&path_str);
            if !path.exists() {
                AppState::Error(format!("File not found: {path_str}"))
            } else {
                match LazyParquetSource::open(path) {
                    Ok(source) => {
                        let file_name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| path_str.clone());
                        AppState::Loaded {
                            file_name,
                            source: Rc::new(source),
                        }
                    }
                    Err(e) => AppState::Error(e),
                }
            }
        }
    };

    application().run(move |cx: &mut App| {
        ui::register_actions(cx);

        let bounds = Bounds::centered(None, size(px(800.), px(600.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Parqueter".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| RootView::new(state, cx));
                    view.focus_handle(cx).focus(window, cx);
                    view
                },
            )
            .unwrap();
        let _ = window;
        cx.activate(true);
    });
}
