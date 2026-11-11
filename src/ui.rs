use std::rc::Rc;

use gpui::{prelude::*, *};

use crate::data::DataSource;

const ROW_NUM_WIDTH: f32 = 60.0;
const ROW_HEIGHT: f32 = 28.0;
const HEADER_HEIGHT: f32 = 58.0;
const SCROLL_STEP: f32 = 100.0;
const SCROLLBAR_SIZE: f32 = 8.0;
const SCROLLBAR_MIN_THUMB: f32 = 20.0;

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
    col_positions: Vec<f32>,
    data_width: f32,
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
        let mut col_positions = Vec::with_capacity(col_widths.len());
        let mut acc = 0.0f32;
        for &w in &col_widths {
            col_positions.push(acc);
            acc += w;
        }
        let data_width = acc;
        Self {
            state,
            col_widths,
            col_positions,
            data_width,
            focus_handle: cx.focus_handle(),
            v_scroll_handle: UniformListScrollHandle::new(),
            scroll_row: 0,
            h_offset: 0.0,
        }
    }

    fn visible_col_range(&self, viewport_width: f32) -> (usize, usize, f32) {
        if self.col_positions.is_empty() {
            return (0, 0, 0.0);
        }
        let left = self.h_offset;
        let right = self.h_offset + viewport_width;

        let first = (0..self.col_widths.len())
            .find(|&i| self.col_positions[i] + self.col_widths[i] > left)
            .unwrap_or(0);

        let last = self
            .col_positions
            .partition_point(|&pos| pos < right)
            .min(self.col_widths.len());

        let spacer = self.col_positions[first] - left;
        (first, last, spacer)
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
        self.h_offset = (self.h_offset + SCROLL_STEP).min(self.data_width);
        window.refresh();
    }

    /// Render a vertical scrollbar. Returns (track, needs_scrollbar).
    fn vertical_scrollbar(&self, track_height: f32) -> Option<Div> {
        let state = self.v_scroll_handle.0.borrow();
        let max_off_f: f32 = state.base_handle.max_offset().y.into();
        if max_off_f <= 0.0 {
            return None;
        }
        let cur_off: f32 = (-state.base_handle.offset().y).into();
        let ratio = (cur_off / max_off_f).clamp(0.0, 1.0);

        // thumb size proportional to viewport/content
        let content_h = track_height + max_off_f;
        let thumb_h = (track_height / content_h * track_height).max(SCROLLBAR_MIN_THUMB);
        let thumb_top = ratio * (track_height - thumb_h);

        Some(
            div()
                .absolute()
                .right_0()
                .top(px(HEADER_HEIGHT))
                .w(px(SCROLLBAR_SIZE))
                .h(px(track_height))
                .child(
                    div()
                        .absolute()
                        .top(px(thumb_top))
                        .w(px(SCROLLBAR_SIZE))
                        .h(px(thumb_h))
                        .rounded(px(SCROLLBAR_SIZE / 2.0))
                        .bg(rgb(0x585b70)),
                ),
        )
    }

    /// Render a horizontal scrollbar.
    fn horizontal_scrollbar(&self, track_width: f32) -> Option<Div> {
        if self.data_width <= track_width {
            return None;
        }
        let ratio = self.h_offset / self.data_width;
        let thumb_w = (track_width / self.data_width * track_width).max(SCROLLBAR_MIN_THUMB);
        let thumb_left = ratio * (track_width - thumb_w);

        Some(
            div()
                .absolute()
                .bottom_0()
                .left(px(ROW_NUM_WIDTH))
                .w(px(track_width))
                .h(px(SCROLLBAR_SIZE))
                .child(
                    div()
                        .absolute()
                        .left(px(thumb_left))
                        .w(px(thumb_w))
                        .h(px(SCROLLBAR_SIZE))
                        .rounded(px(SCROLLBAR_SIZE / 2.0))
                        .bg(rgb(0x585b70)),
                ),
        )
    }
}

impl Focusable for RootView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                let viewport_w: f32 = (window.viewport_size().width - px(ROW_NUM_WIDTH)).into();
                let (vis_first, vis_last, spacer) = self.visible_col_range(viewport_w);
                let vis_widths: Vec<f32> = self.col_widths[vis_first..vis_last].to_vec();

                // File info header
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

                // -- Column header row --
                // Fixed row-num cell + clipped data area (viewport_w wide)
                let header_bg = rgb(0x313244);
                let mut header_data = div().flex().ml(px(spacer));
                for (i, col) in (vis_first..vis_last).enumerate() {
                    header_data = header_data.child(
                        div()
                            .w(px(vis_widths[i]))
                            .h(px(HEADER_HEIGHT))
                            .px_2()
                            .py_1()
                            .border_r_1()
                            .border_b_1()
                            .border_color(rgb(0x45475a))
                            .bg(header_bg)
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
                                    .child(format!(
                                        "{} · col {}",
                                        source.column_type(col),
                                        col + 1
                                    )),
                            ),
                    );
                }

                let header_row = div()
                    .flex()
                    .h(px(HEADER_HEIGHT))
                    .child(
                        // Fixed row-number cell
                        div()
                            .w(px(ROW_NUM_WIDTH))
                            .h(px(HEADER_HEIGHT))
                            .px_2()
                            .flex()
                            .items_center()
                            .border_r_1()
                            .border_b_1()
                            .border_color(rgb(0x45475a))
                            .bg(header_bg)
                            .child(div().text_sm().text_color(rgb(0x6c7086)).child("#")),
                    )
                    .child(
                        // Clipped viewport for header columns
                        div()
                            .w(px(viewport_w))
                            .h(px(HEADER_HEIGHT))
                            .overflow_hidden()
                            .child(header_data),
                    );

                // -- Data rows --
                let source_ref = Rc::clone(source);
                let widths = vis_widths;
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

                                // Data cells
                                let mut data_cells = div().flex().ml(px(spacer));
                                for (i, col_idx) in (vis_first..vis_last).enumerate() {
                                    data_cells = data_cells.child(
                                        div()
                                            .w(px(widths[i]))
                                            .h(px(ROW_HEIGHT))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .border_r_1()
                                            .border_color(rgb(0x45475a))
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(rgb(0xcdd6f4))
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                                    .child(
                                                        source_ref.cell(row_idx, col_idx),
                                                    ),
                                            ),
                                    );
                                }

                                div()
                                    .flex()
                                    .h(px(ROW_HEIGHT))
                                    .bg(bg)
                                    .child(
                                        // Fixed row-number cell
                                        div()
                                            .w(px(ROW_NUM_WIDTH))
                                            .h(px(ROW_HEIGHT))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .border_r_1()
                                            .border_color(rgb(0x45475a))
                                            .bg(bg)
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x585b70))
                                                    .child(format!("{}", row_idx + 1)),
                                            ),
                                    )
                                    .child(
                                        // Clipped viewport for data columns
                                        div()
                                            .w(px(viewport_w))
                                            .h(px(ROW_HEIGHT))
                                            .overflow_hidden()
                                            .child(data_cells),
                                    )
                            })
                            .collect()
                    },
                )
                .flex_1()
                .track_scroll(&self.v_scroll_handle);

                let viewport_h: f32 = (window.viewport_size().height - px(HEADER_HEIGHT)).into();
                let body_h = viewport_h - HEADER_HEIGHT; // approximate data area height

                let body = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(header_row)
                    .child(data_rows);

                // Main layout with scrollbar overlays
                let mut table_area = div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(body);

                if let Some(v_bar) = self.vertical_scrollbar(body_h) {
                    table_area = table_area.child(v_bar);
                }
                if let Some(h_bar) = self.horizontal_scrollbar(viewport_w) {
                    table_area = table_area.child(h_bar);
                }

                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(file_header)
                    .child(table_area)
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
