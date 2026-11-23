//! The graph band at the top and scrolling back through history.

use super::*;

impl RepoView {
    /// Whether the graph shows the First and Latest buttons.
    pub(super) fn graph_away_now(&self) -> (bool, bool) {
        let full_width = self.graph_width.get() + 2. * graph_view::MARGIN;
        let view = f32::from(self.graph_scroll.bounds().size.width);
        let scrolled = -f32::from(self.graph_scroll.offset().x);
        let shown = view > 0. && !self.graph.nodes.is_empty();
        // First at the left edge, while older commits are still to load;
        // Latest only when the newest work is more than two views away.
        (
            shown && scrolled <= 2. && self.graph.older > 0,
            shown && full_width - view - scrolled > 2. * view,
        )
    }

    /// Loads the whole history of the main branch and scrolls to its first
    /// commit.
    fn go_to_first_commit(&mut self, cx: &mut Context<Self>) {
        self.scroll_to_start.set(true);
        if self.graph.older > 0 && !self.loading_older {
            self.loading_older = true;
            self.history += self.graph.older;
            self.start_refresh(cx);
        }
        cx.notify();
    }

    /// Brings older commits into the graph, a step further back each time.
    pub fn load_older(&mut self, cx: &mut Context<Self>) {
        if self.loading_older || self.graph.older == 0 {
            return;
        }
        self.loading_older = true;
        self.history += HISTORY_STEP;
        self.start_refresh(cx);
        cx.notify();
    }

    /// The graph's layout, made again only when the graph or what shapes it
    /// changes, not on every frame.
    fn graph_layout(&self) -> Rc<graph_view::Layout> {
        let key = (
            self.worktree_order,
            preferences::graph(),
            preferences::idle_days(),
            preferences::sync_badges(),
            now_seconds() / 3_600,
        );
        let mut cache = self.graph_layout.borrow_mut();
        if let Some((cached, layout)) = cache.as_ref()
            && *cached == key
        {
            return layout.clone();
        }
        let layout = Rc::new(graph_view::layout(&self.graph, self.worktree_order));
        *cache = Some((key, layout.clone()));
        layout
    }

    pub(super) fn graph_band(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if let Some(error) = &self.graph_error {
            return placeholder("Cannot read the repository", Some(error.clone()), theme);
        }
        let layout = self.graph_layout();
        // When the graph grows or shrinks, as attention changes, the view keeps
        // its distance from the newest work on the right instead of jumping.
        let previous = self.graph_width.replace(layout.width);
        if previous > 0. && previous != layout.width {
            let offset = self.graph_scroll.offset();
            let x = offset.x - px(layout.width - previous);
            self.graph_scroll.set_offset(point(x.min(px(0.)), offset.y));
        }
        // A node selected from the commit list or Go To scrolls into view
        // when it is out of it; one picked in the graph already is in view.
        let view = self.graph_scroll.bounds().size;
        // A repository opens with the graph scrolled to its newest work.
        if view.width > px(0.) && !self.graph.nodes.is_empty() && self.scroll_to_end.replace(false)
        {
            let width = layout.width + 2. * graph_view::MARGIN;
            let x = (width - f32::from(view.width)).max(0.);
            self.graph_scroll.set_offset(point(px(-x), px(0.)));
        }
        if let Some((start, end, y)) =
            self.reveal
                .get()
                .filter(|_| view.width > px(0.))
                .and_then(|node| {
                    self.reveal.set(None);
                    layout.nodes.get(node).copied()
                })
        {
            // In the scrolled content, the graph sits inside its margin.
            let pad = graph_view::MARGIN;
            let (start, end, y) = (start + pad, end + pad, y + pad);
            let (full_width, full_height) = (layout.width + 2. * pad, layout.height + 2. * pad);
            let (width, height) = (f32::from(view.width), f32::from(view.height));
            let offset = self.graph_scroll.offset();
            let (left, top) = (-f32::from(offset.x), -f32::from(offset.y));
            let margin = 24.;
            let x = match start < left + margin || end > left + width - margin {
                true => ((start + end - width) / 2.).clamp(0., (full_width - width).max(0.)),
                false => left,
            };
            let y = match y < top + margin || y > top + height - margin {
                true => (y - height / 2.).clamp(0., (full_height - height).max(0.)),
                false => top,
            };
            self.graph_scroll.set_offset(point(px(-x), px(-y)));
        }
        let height = self
            .graph_height
            .unwrap_or(layout.height.clamp(MIN_GRAPH_HEIGHT, MAX_GRAPH_HEIGHT));
        // Scrolled away from either end, a button goes there.
        if self.graph.older == 0 && !self.loading_older && self.scroll_to_start.replace(false) {
            let offset = self.graph_scroll.offset();
            self.graph_scroll.set_offset(point(px(0.), offset.y));
        }
        let (first, away) = self.graph_away_now();
        self.graph_away.set((first, away));
        let picked: Vec<String> = self
            .narrowed
            .iter()
            .filter_map(|&i| self.commits.get(i))
            .map(|commit| commit.id.clone())
            .collect();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .h(px(height))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .bg(theme.graph_background)
                    .on_scroll_wheel(cx.listener(Self::pull_older))
                    // A press on nothing in particular grabs the graph to drag.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            if this.hovered_node.is_none() {
                                this.graph_drag =
                                    Some((event.position, this.graph_scroll.offset()));
                                cx.notify();
                            }
                        }),
                    )
                    .when(self.graph_drag.is_some(), |graph| {
                        graph.cursor(CursorStyle::ClosedHand)
                    })
                    .relative()
                    .child(div().flex_1().min_w_0().h_full().child(graph_view::render(
                        &self.graph,
                        &layout,
                        graph_view::Marks {
                            selected: &self.selected_nodes,
                            picked: &picked,
                            pull: match self.loading_older {
                                true => Some(1.),
                                false => self.pull.get().map(|pull| pull / PULL_DISTANCE),
                            },
                            hovered: self.hovered_node,
                            stash: self.stash_view.as_ref().map(|stash| stash.name.as_str()),
                        },
                        &self.graph_scroll,
                        theme,
                        cx,
                    )))
                    .when(away, |graph| {
                        graph.child(
                            graph_button("graph-latest", "Latest →", theme)
                                .right(px(10.))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.scroll_to_end.set(true);
                                    cx.notify();
                                })),
                        )
                    })
                    .when(first, |graph| {
                        let label = match self.loading_older && self.scroll_to_start.get() {
                            true => "Loading history…",
                            false => "← First commit",
                        };
                        graph.child(
                            graph_button("graph-first", label, theme)
                                .left(px(10.))
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.go_to_first_commit(cx)),
                                ),
                        )
                    }),
            )
            .child(border_handle("graph-border", theme).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dragging = Some(Border::Graph);
                    cx.stop_propagation();
                }),
            ))
            .into_any_element()
    }
}
