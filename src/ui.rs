use std::rc::Rc;

use gpui::{prelude::*, *};

use crate::data::DataSource;

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
    col_widths: Vec<f32>,
    focus_handle: FocusHandle,
    v_scroll_handle: UniformListScrollHandle,
    scroll_row: usize,
    h_offset: f32,
}

impl RootView {
    pub fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let col_widths = match &state {
            AppState::Loaded { source, .. } => source.column_widths(),
            _ => vec![],
        };
        Self {
            state,
            col_widths,
            focus_handle: cx.focus_handle(),
            v_scroll_handle: UniformListScrollHandle::new(),
            scroll_row: 0,
            h_offset: 0.0,
        }
    }

    fn data_width(&self) -> f32 {
        self.col_widths.iter().sum()
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
        self.h_offset = (self.h_offset - SCROLL_STEP).max(0.0);
        window.refresh();
    }

    fn scroll_right(&mut self, _: &ScrollRight, window: &mut Window, _cx: &mut Context<Self>) {
        let max_offset = self.data_width();
        self.h_offset = (self.h_offset + SCROLL_STEP).min(max_offset);
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
                let col_widths = &self.col_widths;
                let data_width: f32 = col_widths.iter().sum();
                let h_offset = self.h_offset;

                // File info header (fixed, outside scroll)
                let file_header = div()
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

                // -- Fixed row-number column header (#) --
                let row_num_header = div()
                    .w(px(ROW_NUM_WIDTH))
                    .h(px(48.0))
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
                    );

                // -- Scrollable column headers (inside h-scroll) --
                let mut col_headers = div().flex().h(px(48.0));
                for col in 0..col_count {
                    col_headers = col_headers.child(
                        div()
                            .w(px(col_widths[col]))
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

                // Combined header row: fixed # + scrollable column headers
                let header_row = div()
                    .flex()
                    .child(row_num_header)
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .child(
                                div()
                                    .w(px(data_width))
                                    .ml(px(-h_offset))
                                    .child(col_headers),
                            ),
                    );

                // -- Data rows via uniform_list --
                // Each row: fixed row-number cell + scrollable data cells
                let source_ref = Rc::clone(source);
                let widths = col_widths.clone();
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

                                // Data cells (will be inside h-scroll)
                                let mut data_div = div().flex().h(px(ROW_HEIGHT));
                                for col_idx in 0..col_count {
                                    data_div = data_div.child(
                                        div()
                                            .w(px(widths[col_idx]))
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
                                                            .cell(row_idx, col_idx),
                                                    ),
                                            ),
                                    );
                                }

                                // Fixed row number + data cells
                                div()
                                    .flex()
                                    .h(px(ROW_HEIGHT))
                                    .bg(bg)
                                    .child(
                                        div()
                                            .w(px(ROW_NUM_WIDTH))
                                            .h(px(ROW_HEIGHT))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .border_r_1()
                                            .border_color(rgb(0x313244))
                                            .bg(bg)
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x585b70))
                                                    .child(format!("{}", row_idx + 1)),
                                            ),
                                    )
                                    .child(
                                        div().flex_1().overflow_hidden().child(
                                            div()
                                                .w(px(data_width))
                                                .ml(px(-h_offset))
                                                .child(data_div),
                                        ),
                                    )
                            })
                            .collect()
                    },
                )
                .flex_1()
                .track_scroll(&self.v_scroll_handle);

                // Body area: header row + data rows (both share the same h-scroll offset)
                // We sync horizontal scroll by applying the offset as a negative margin
                // on the scrollable portions via the h-scroll container
                let body = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(header_row)
                    .child(data_rows);

                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(file_header)
                    .child(body)
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
