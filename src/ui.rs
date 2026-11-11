use std::rc::Rc;

use gpui::{prelude::*, *};

use crate::data::DataSource;

const COLUMN_WIDTH: f32 = 150.0;
const ROW_NUM_WIDTH: f32 = 60.0;
const ROW_HEIGHT: f32 = 28.0;
const SCROLL_STEP: f32 = 100.0;

actions!(parqueter, [ScrollUp, ScrollDown, ScrollLeft, ScrollRight]);

pub fn register_actions(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", ScrollUp, None),
        KeyBinding::new("down", ScrollDown, None),
        KeyBinding::new("left", ScrollLeft, None),
        KeyBinding::new("right", ScrollRight, None),
    ]);
}

pub enum AppState {
    Loaded {
        file_name: String,
        source: Rc<dyn DataSource>,
    },
    Error(String),
}

pub struct RootView {
    state: AppState,
    focus_handle: FocusHandle,
    v_scroll_handle: UniformListScrollHandle,
    h_scroll_handle: ScrollHandle,
    scroll_row: usize,
}

impl RootView {
    pub fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        Self {
            state,
            focus_handle: cx.focus_handle(),
            v_scroll_handle: UniformListScrollHandle::new(),
            h_scroll_handle: ScrollHandle::new(),
            scroll_row: 0,
        }
    }

    fn row_count(&self) -> usize {
        match &self.state {
            AppState::Loaded { source, .. } => source.row_count(),
            _ => 0,
        }
    }

    fn scroll_up(&mut self, _: &ScrollUp, window: &mut Window, _cx: &mut Context<Self>) {
        self.scroll_row = self.scroll_row.saturating_sub(5);
        self.v_scroll_handle
            .scroll_to_item(self.scroll_row, ScrollStrategy::Top);
        window.refresh();
    }

    fn scroll_down(&mut self, _: &ScrollDown, window: &mut Window, _cx: &mut Context<Self>) {
        let max = self.row_count().saturating_sub(1);
        self.scroll_row = (self.scroll_row + 5).min(max);
        self.v_scroll_handle
            .scroll_to_item(self.scroll_row, ScrollStrategy::Top);
        window.refresh();
    }

    fn scroll_left(&mut self, _: &ScrollLeft, window: &mut Window, _cx: &mut Context<Self>) {
        let offset = self.h_scroll_handle.offset();
        self.h_scroll_handle.set_offset(point(
            offset.x + px(SCROLL_STEP),
            offset.y,
        ));
        window.refresh();
    }

    fn scroll_right(&mut self, _: &ScrollRight, window: &mut Window, _cx: &mut Context<Self>) {
        let offset = self.h_scroll_handle.offset();
        self.h_scroll_handle.set_offset(point(
            offset.x - px(SCROLL_STEP),
            offset.y,
        ));
        window.refresh();
    }
}

impl Focusable for RootView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                )
                .into_any_element(),

            AppState::Loaded { file_name, source } => {
                let col_count = source.col_count();
                let row_count = source.row_count();
                let table_width = ROW_NUM_WIDTH + col_count as f32 * COLUMN_WIDTH;

                // File info header (fixed, outside scroll)
                let header = div()
                    .pb_2()
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
                            .child(format!("{col_count} columns · {row_count} rows")),
                    );

                // Column headers row
                let mut header_row = div().flex().h(px(48.0))
                    .child(
                        div()
                            .w(px(ROW_NUM_WIDTH))
                            .px_2()
                            .py_1()
                            .border_r_1()
                            .border_b_1()
                            .border_color(rgb(0x45475a))
                            .bg(rgb(0x313244))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x6c7086))
                                    .child("#"),
                            ),
                    );
                for col in 0..col_count {
                    header_row = header_row.child(
                        div()
                            .w(px(COLUMN_WIDTH))
                            .px_2()
                            .py_1()
                            .border_r_1()
                            .border_b_1()
                            .border_color(rgb(0x45475a))
                            .bg(rgb(0x313244))
                            .overflow_hidden()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x89b4fa))
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(source.column_name(col).to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x6c7086))
                                    .child(source.column_type(col).to_string()),
                            ),
                    );
                }

                // Data rows via uniform_list (virtualized vertical scroll)
                let source_ref = Rc::clone(source);
                let data_rows = uniform_list(
                    "data-rows",
                    row_count,
                    move |range, _window, _cx| {
                        range
                            .map(|row_idx| {
                                let bg = if row_idx % 2 == 0 {
                                    rgb(0x1e1e2e)
                                } else {
                                    rgb(0x181825)
                                };
                                let mut row_div = div().flex().h(px(ROW_HEIGHT)).bg(bg)
                                    .child(
                                        div()
                                            .w(px(ROW_NUM_WIDTH))
                                            .h(px(ROW_HEIGHT))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .border_r_1()
                                            .border_color(rgb(0x313244))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x585b70))
                                                    .child(format!("{}", row_idx + 1)),
                                            ),
                                    );
                                for col_idx in 0..col_count {
                                    row_div = row_div.child(
                                        div()
                                            .w(px(COLUMN_WIDTH))
                                            .h(px(ROW_HEIGHT))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .border_r_1()
                                            .border_color(rgb(0x313244))
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(rgb(0xcdd6f4))
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                                    .child(
                                                        source_ref
                                                            .cell(row_idx, col_idx)
                                                            .to_string(),
                                                    ),
                                            ),
                                    );
                                }
                                row_div
                            })
                            .collect()
                    },
                )
                .w(px(table_width))
                .flex_1()
                .track_scroll(&self.v_scroll_handle);

                // Wide inner container for horizontal scroll
                let wide_inner = div()
                    .w(px(table_width))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(header_row)
                    .child(data_rows);

                // Horizontal scroll container
                let h_scroll = div()
                    .id("h-scroll")
                    .overflow_x_scroll()
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.h_scroll_handle)
                    .child(wide_inner);

                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(header)
                    .child(h_scroll)
                    .into_any_element()
            }
        };

        div()
            .id("root")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::scroll_up))
            .on_action(cx.listener(Self::scroll_down))
            .on_action(cx.listener(Self::scroll_left))
            .on_action(cx.listener(Self::scroll_right))
            .size_full()
            .bg(rgb(0x1e1e2e))
            .child(content)
    }
}
