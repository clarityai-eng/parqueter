use gpui::*;

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
                let mut col = div().flex().flex_col().size_full().p_4().gap_2();

                // Header with file name
                col = col.child(
                    div()
                        .pb_2()
                        .mb_2()
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
                        ),
                );

                // Column header row
                col = col.child(
                    div()
                        .flex()
                        .px_3()
                        .py_1()
                        .text_sm()
                        .text_color(rgb(0x6c7086))
                        .child(div().flex_1().child("Name"))
                        .child(div().w(px(200.)).child("Type")),
                );

                // Column rows
                for (i, column) in columns.iter().enumerate() {
                    let bg = if i % 2 == 0 {
                        rgb(0x1e1e2e)
                    } else {
                        rgb(0x181825)
                    };
                    col = col.child(
                        div()
                            .flex()
                            .px_3()
                            .py_1()
                            .rounded_sm()
                            .bg(bg)
                            .child(
                                div()
                                    .flex_1()
                                    .text_color(rgb(0xcdd6f4))
                                    .child(column.name.clone()),
                            )
                            .child(
                                div()
                                    .w(px(200.))
                                    .text_color(rgb(0xa6adc8))
                                    .child(column.data_type.clone()),
                            ),
                    );
                }

                col
            }
        };

        div().size_full().bg(rgb(0x1e1e2e)).child(content)
    }
}
