mod data;
mod ui;

use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures::StreamExt;
use futures::channel::mpsc;
use gpui::*;
use gpui_platform::application;

use crate::data::LazyParquetSource;
use crate::ui::{AppState, RootView};

fn main() {
    // Ignore the legacy `-psn_*` argument older macOS versions pass when launching from Finder.
    let state = match std::env::args().skip(1).find(|a| !a.starts_with("-psn_")) {
        None => AppState::Error("Drop a file or press Ctrl+O to open a Parquet file".to_string()),
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

    // macOS delivers files opened from Finder (double-click, "Open With", drag onto the
    // Dock icon) as an open-URLs event, not as argv. The callback has no `App` context,
    // so forward the paths through a channel and handle them inside the app.
    let (open_tx, mut open_rx) = mpsc::unbounded::<PathBuf>();
    let app = application();
    app.on_open_urls(move |urls| {
        for url in urls {
            if let Some(path) = file_url_to_path(&url) {
                open_tx.unbounded_send(path).ok();
            }
        }
    });

    app.run(move |cx: &mut App| {
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

        cx.spawn(async move |cx| {
            while let Some(path) = open_rx.next().await {
                window
                    .update(cx, |view, window, cx| {
                        view.load_file(&path, window, cx);
                        window.activate_window();
                    })
                    .ok();
            }
        })
        .detach();

        cx.activate(true);
    });
}

fn file_url_to_path(url: &str) -> Option<PathBuf> {
    match url::Url::parse(url) {
        Ok(parsed) if parsed.scheme() == "file" => parsed.to_file_path().ok(),
        Ok(_) => None,
        // Not a URL: treat it as a plain path.
        Err(_) => Some(PathBuf::from(url)),
    }
}
