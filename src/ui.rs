use gpui::{prelude::*, *};

use crate::schema::ColumnInfo;

pub enum AppState {
    Loaded {
        file_name: String,
        columns: Vec<ColumnInfo>,
    },
    Error(String),
}

pub struct RootView {
    state: AppState,
}

impl RootView {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.state {
            AppState::Error(msg) => div()
                .flex()
                .flex_col()
                .size_full()
                .justify_center()
                .items_center()
                .child(
                    div()
                        .text_color(rgb(0xf38ba8))
                        .text_xl()
                        .child(msg.clone()),
                ),
            AppState::Loaded { file_name, columns } => {
                // Header with file name (fixed, not scrollable)
                let header = div()
                    .pb_2()
                    .mb_2()
                    .px_4()
                    .pt_4()
                    .border_b_1()
                    .border_color(rgb(0x45475a))
                    .child(
                        div()
                            .text_xl()
                            .text_color(rgb(0x89b4fa))
                            .child(file_name.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x6c7086))
                            .child(format!("{} columns", columns.len())),
                    );

                // Column cards in a wrapping grid with scroll
                let mut grid = div()
                    .id("column-grid")
                    .overflow_y_scroll()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .p_4()
                    .flex_1();

                for (i, column) in columns.iter().enumerate() {
                    let bg = if i % 2 == 0 {
                        rgb(0x313244)
                    } else {
                        rgb(0x2a2b3d)
                    };
                    grid = grid.child(
                        div()
                            .w(px(200.))
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(bg)
                            .child(
                                div()
                                    .text_color(rgb(0xcdd6f4))
                                    .text_sm()
                                    .child(column.name.clone()),
                            )
                            .child(
                                div()
                                    .text_color(rgb(0x6c7086))
                                    .text_xs()
                                    .child(column.data_type.clone()),
                            ),
                    );
                }

                div().flex().flex_col().size_full().child(header).child(grid)
            }
        };

        div().size_full().bg(rgb(0x1e1e2e)).child(content)
    }
}
