//! The graph band at the top and scrolling back through history.

use super::*;

/// Which edge a worktree row runs past, and where a click there goes: the
/// node its work starts at, or where its name sits in the list.
#[derive(Clone, Copy)]
enum Edge {
    Left(usize),
    Right(f32),
}

/// How tall the chips at the graph's edges are.
const EDGE_CHIP: f32 = 18.;
/// How near the right edge a worktree's name starts and still counts as out
/// of view.
const EDGE_ROOM: f32 = 24.;
/// How much of the worktree list scrolling to it shows.
const LIST_ROOM: f32 = 220.;

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

    /// Selects the node an arrow key points to from the selected one, as
    /// the graph is drawn: the nearest that way, nodes on its row or lane
    /// first. With nothing selected, it starts from the current worktree or
    /// the main branch. False when there is nothing that way.
    pub(super) fn step_node(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let layout = self.graph_layout();
        let from = self
            .selected_nodes
            .last()
            .copied()
            .or_else(|| self.current_worktree())
            .or(self.graph.main_node);
        let Some(&(start, end, y)) = from.and_then(|from| layout.nodes.get(from)) else {
            return false;
        };
        let x = (start + end) / 2.;
        let next = graph_view::nearest(&layout.nodes, x, y, key);
        match next {
            Some(next) if Some(next) != from => {
                self.select_and_reveal(next, cx);
                true
            }
            // At the left edge, older commits come in, as scrolling on past
            // it brings them; the next press goes on into them.
            None if key == "left" && self.graph.older > 0 => {
                self.load_older(cx);
                false
            }
            _ => false,
        }
    }

    /// ⌥← and ⌥→: the selected node's commits one by one, older or
    /// `newer`, as clicks on its beads pick them. Past either end of its
    /// run, the next node that way, at its nearer end.
    pub(super) fn step_commit(&mut self, newer: bool, cx: &mut Context<Self>) {
        let key = if newer { "right" } else { "left" };
        let [node] = self.selected_nodes[..] else {
            self.step_node(key, cx);
            return;
        };
        // Oldest first, as the beads run.
        let ids: Vec<String> = self.graph.nodes[node]
            .commits
            .iter()
            .map(|commit| commit.id.clone())
            .collect();
        let current = self
            .pending_commit
            .clone()
            .or_else(|| match self.narrowed[..] {
                [i] => self.commits.get(i).map(|commit| commit.id.clone()),
                _ => None,
            });
        let at = current.and_then(|id| ids.iter().position(|c| *c == id));
        let next = match (at, newer) {
            // A lone commit is its node already.
            (None, _) if ids.len() <= 1 => None,
            (None, true) => Some(0),
            (None, false) => Some(ids.len() - 1),
            (Some(at), true) => (at + 1 < ids.len()).then_some(at + 1),
            (Some(at), false) => at.checked_sub(1),
        };
        if let Some(i) = next {
            self.select_commit(node, ids[i].clone(), cx);
            return;
        }
        if self.step_node(key, cx)
            && let [next] = self.selected_nodes[..]
        {
            let commits = &self.graph.nodes[next].commits;
            let end = if newer {
                commits.first()
            } else {
                commits.last()
            };
            if let Some(commit) = end {
                self.select_commit(next, commit.id.clone(), cx);
            }
        }
        cx.notify();
    }

    /// The current worktree's node: the last one selected, else the one
    /// this tab opened.
    pub(super) fn current_worktree(&self) -> Option<usize> {
        let path = self.command_path()?;
        let path = path.trim_end_matches('/');
        self.graph.nodes.iter().position(
            |node| matches!(&node.tip, Revision::Worktree(p) if p.trim_end_matches('/') == path),
        )
    }

    /// The current worktree, when the graph is scrolled away from it, and
    /// the arrow pointing to it.
    pub(super) fn worktree_away(
        &self,
        layout: &graph_view::Layout,
    ) -> Option<(usize, &'static str)> {
        let w = self.current_worktree()?;
        let (start, end, y) = *layout.nodes.get(w)?;
        let pad = graph_view::MARGIN;
        let (start, end, y) = (start + pad, end + pad, y + pad);
        let view = self.graph_scroll.bounds().size;
        let (width, height) = (f32::from(view.width), f32::from(view.height));
        if width <= 0. {
            return None;
        }
        let offset = self.graph_scroll.offset();
        let (left, top) = (-f32::from(offset.x), -f32::from(offset.y));
        let arrow = if end < left {
            "←"
        } else if start > left + width {
            "→"
        } else if y < top {
            "↑"
        } else if y > top + height {
            "↓"
        } else {
            return None;
        };
        Some((w, arrow))
    }

    /// The current, selected or hovered worktrees' rows in view whose ends
    /// are out of it: on the left, where
    /// the work lies past the view's left edge, with the node it starts at;
    /// on the right, where the worktree's name in the list lies past the
    /// right edge, with where the name is.
    fn rows_out_of_view(&self, layout: &graph_view::Layout) -> Vec<(usize, f32, Edge)> {
        let view = self.graph_scroll.bounds().size;
        let (width, height) = (f32::from(view.width), f32::from(view.height));
        if width <= 0. {
            return Vec::new();
        }
        let offset = self.graph_scroll.offset();
        let (left, top) = (-f32::from(offset.x), -f32::from(offset.y));
        let pad = graph_view::MARGIN;
        // Only for the worktree in hand: the current one, one selected, or
        // one the pointer is on.
        let current = self.current_worktree();
        let wanted = |w: usize| {
            current == Some(w) || self.selected_nodes.contains(&w) || self.hovered_node == Some(w)
        };
        let mut rows = Vec::new();
        for (_, y, group) in &layout.leads {
            let Some(&w) = group.iter().find(|&&w| wanted(w)) else {
                continue;
            };
            let y = y + pad - top;
            // Whole, or not at all: a chip cut by the band's edge reads as
            // something else.
            if y < EDGE_CHIP / 2. || y > height - EDGE_CHIP / 2. {
                continue;
            }
            // Its mark, and the labels before it, all out of view.
            if layout.nodes[w].1 + pad < left {
                rows.push((w, y, Edge::Left(self.work_start(w))));
            }
            if let Some(x) = layout.list_x[w].filter(|x| x + pad > left + width - EDGE_ROOM) {
                rows.push((w, y, Edge::Right(x)));
            }
        }
        rows
    }

    /// Where a worktree's own work starts: back from its commit along its
    /// lane, short of the main line and release lines.
    fn work_start(&self, worktree: usize) -> usize {
        let nodes = &self.graph.nodes;
        let lane = nodes[worktree].lane;
        let mut at = nodes[worktree].parents.first().copied().unwrap_or(worktree);
        while let Some(&parent) = nodes[at].parents.first() {
            let node = &nodes[parent];
            let main = self.graph.on_main.get(parent).copied().unwrap_or(false);
            if node.lane != lane || main || node.channel {
                break;
            }
            at = parent;
        }
        at
    }

    /// At the view's edges, on each worktree row whose ends are out of
    /// view, a chevron over the graph pointing the way; while the pointer is
    /// over it, or over the worktree, it names the branch. A click goes
    /// there: to the work's start on the left, to the worktree's name on the
    /// right.
    fn edge_arrows(
        &self,
        layout: &graph_view::Layout,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.rows_out_of_view(layout)
            .into_iter()
            .map(|(w, y, edge)| {
                let node = &self.graph.nodes[w];
                let color = theme.lanes[node.lane % theme.lanes.len()];
                let left = matches!(edge, Edge::Left(_));
                let key = (w, left);
                let open = self.edge_hovered == Some(key) || self.hovered_node == Some(w);
                let name = match left {
                    true => node
                        .branch
                        .clone()
                        .unwrap_or_else(|| graph_view::worktree_label(node, self.worktree_names)),
                    false => graph_view::worktree_label(node, self.worktree_names),
                };
                let chevron = graph_view::chevron(left, if open { gpui::white() } else { color });
                let text = open.then(|| div().child(name));
                div()
                    .id(("edge-arrow", w * 2 + usize::from(left)))
                    .absolute()
                    .map(|chip| match left {
                        true => chip.left(px(4.)),
                        false => chip.right(px(4.)),
                    })
                    .top(px(y - EDGE_CHIP / 2.))
                    .h(px(EDGE_CHIP))
                    .min_w(px(EDGE_CHIP))
                    .px(px(5.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(5.))
                    .rounded(px(EDGE_CHIP / 2.))
                    .border_1()
                    .shadow_sm()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .map(|chip| match open {
                        true => chip.bg(color).border_color(color).text_color(gpui::white()),
                        false => chip
                            .bg(theme.graph_background)
                            .border_color(color.opacity(0.6)),
                    })
                    .map(|chip| match left {
                        true => chip.child(chevron).children(text),
                        false => chip.children(text).child(chevron),
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.edge_hovered = match hovered {
                            true => Some(key),
                            false => this.edge_hovered.filter(|&at| at != key),
                        };
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_scroll_wheel(cx.listener(Self::scroll_graph))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edge_hovered = None;
                        match edge {
                            Edge::Left(start) => this.reveal.set(Some(start)),
                            Edge::Right(x) => this.scroll_to_list(x),
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// Scrolls the graph so the worktree list, at `x`, comes into view at
    /// the right, its names readable.
    fn scroll_to_list(&self, x: f32) {
        let view = f32::from(self.graph_scroll.bounds().size.width);
        let max = f32::from(self.graph_scroll.max_offset().width);
        let target = (x + graph_view::MARGIN + LIST_ROOM - view).clamp(0., max);
        let offset = self.graph_scroll.offset();
        self.graph_scroll.set_offset(point(px(-target), offset.y));
    }

    /// The arrow to the current worktree, out of view: at the edge it lies
    /// past, level with it, quiet until the pointer is over it, then with
    /// its name.
    fn worktree_arrow(
        &self,
        w: usize,
        arrow: &'static str,
        layout: &graph_view::Layout,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let node = &self.graph.nodes[w];
        let color = theme.lanes[node.lane % theme.lanes.len()];
        let name = graph_view::worktree_label(node, self.worktree_names);
        let key = (w, false);
        let open = self.edge_hovered == Some(key) || self.hovered_node == Some(w);
        let (_, end, y) = layout.nodes[w];
        let pad = graph_view::MARGIN;
        let view = self.graph_scroll.bounds().size;
        let (width, height) = (f32::from(view.width), f32::from(view.height));
        let offset = self.graph_scroll.offset();
        // Where it is, in the view, held inside the view's edges.
        let x = (end + pad + f32::from(offset.x)).clamp(12., (width - 12.).max(12.));
        let y = (y + pad + f32::from(offset.y)).clamp(12., (height - 12.).max(12.));
        let (glyph, label) = match arrow {
            "←" => ("‹", format!("‹ {name}")),
            "→" => ("›", format!("{name} ›")),
            "↑" => ("↑", format!("↑ {name}")),
            _ => ("↓", format!("↓ {name}")),
        };
        let chip = div()
            .id("graph-worktree")
            .absolute()
            .h(px(20.))
            .flex()
            .items_center()
            .whitespace_nowrap()
            .cursor_pointer()
            .text_xs()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .map(|chip| match open {
                true => chip
                    .px(px(8.))
                    .rounded(px(10.))
                    .bg(color)
                    .text_color(gpui::white())
                    .child(label),
                false => chip
                    .px(px(4.))
                    .text_color(color)
                    .text_size(px(match arrow {
                        "←" | "→" => 18.,
                        _ => 14.,
                    }))
                    .child(glyph),
            })
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.edge_hovered = match hovered {
                    true => Some(key),
                    false => this.edge_hovered.filter(|&at| at != key),
                };
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(cx.listener(Self::scroll_graph))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.edge_hovered = None;
                this.reveal.set(Some(w));
                cx.notify();
            }));
        match arrow {
            "←" => chip.left(px(4.)).top(px(y - 10.)),
            "→" => chip.right(px(4.)).top(px(y - 10.)),
            // Up or down, it grows away from the nearer side edge.
            "↑" => chip.top(px(4.)),
            _ => chip.bottom(px(4.)),
        }
        .map(|chip| match arrow {
            "←" | "→" => chip,
            _ if x > width / 2. => chip.right(px(width - x - 10.)),
            _ => chip.left(px(x - 10.)),
        })
        .into_any_element()
    }

    /// Scrolls the graph from a button floating over it, as if the button
    /// were not there.
    fn scroll_graph(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(px(20.));
        let offset = self.graph_scroll.offset();
        let max = self.graph_scroll.max_offset();
        let x = (offset.x + delta.x).clamp(-max.width, px(0.));
        let y = (offset.y + delta.y).clamp(-max.height, px(0.));
        self.graph_scroll.set_offset(point(x, y));
        cx.stop_propagation();
        cx.notify();
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
    pub(super) fn graph_layout(&self) -> Rc<graph_view::Layout> {
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

    pub(super) fn graph_band(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        // When the view narrows or widens, as a side panel opens or closes,
        // the graph keeps its right edge where it was: what was at the right
        // stays in view, and the rest moves over.
        let view_width = f32::from(self.graph_scroll.bounds().size.width);
        let was = self.graph_view_width.replace(view_width);
        if was > 0. && view_width > 0. && was != view_width {
            let offset = self.graph_scroll.offset();
            // The scroll range is still the old view's: narrower, it grows.
            let max = self.graph_scroll.max_offset().width + px((was - view_width).max(0.));
            let x = (offset.x - px(was - view_width)).clamp(-max, px(0.));
            self.graph_scroll.set_offset(point(x, offset.y));
            // The view's new width is known only once drawn: draw again.
            cx.notify();
        }
        // A node selected from the commit list or Go To scrolls into view
        // when it is out of it; one picked in the graph already is in view.
        let view = self.graph_scroll.bounds().size;
        // A repository opens with the graph scrolled to its newest work, its
        // top and right a little in from the band's edges; the margin past
        // them is there to scroll into.
        if view.width > px(0.) && !self.graph.nodes.is_empty() && self.scroll_to_end.replace(false)
        {
            let hidden = graph_view::MARGIN - GRAPH_EDGE;
            let width = layout.width + 2. * graph_view::MARGIN;
            let x = (width - f32::from(view.width) - hidden).max(0.);
            let height = layout.height + 2. * graph_view::MARGIN;
            let y = hidden.min((height - f32::from(view.height)).max(0.));
            self.graph_scroll.set_offset(point(px(-x), px(-y)));
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
        // As tall as the graph, up to a share of the window; then it scrolls.
        let most = f32::from(window.viewport_size().height) * MAX_GRAPH_SHARE;
        let height = self.graph_height.unwrap_or(
            (layout.height + 2. * GRAPH_EDGE)
                .clamp(MIN_PANEL_HEIGHT.min(most), most.max(MIN_PANEL_HEIGHT)),
        );
        // Scrolled away from either end, a button goes there.
        if self.graph.older == 0 && !self.loading_older && self.scroll_to_start.replace(false) {
            let offset = self.graph_scroll.offset();
            self.graph_scroll.set_offset(point(px(0.), offset.y));
        }
        let (first, away) = self.graph_away_now();
        self.graph_away.set((first, away));
        let edges = self.edge_arrows(&layout, theme, cx);
        // On a row in view, the worktree has its edge chevrons already; the
        // arrow to it is for when its row is above or below the view.
        let worktree = self
            .worktree_away(&layout)
            .filter(|&(_, arrow)| arrow == "↑" || arrow == "↓");
        self.worktree_arrow.set(worktree.map(|(_, arrow)| arrow));
        // Scrolled away from the current worktree, an arrow at the edge on
        // its way points to it, as the leads' arrows do; a click goes there.
        let worktree_away =
            worktree.map(|(w, arrow)| self.worktree_arrow(w, arrow, &layout, theme, cx));
        let away = away && worktree_away.is_none();
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
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            // The arrow keys then move about the graph.
                            this.focus.focus(window);
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
                            current: self.current_worktree(),
                        },
                        &self.graph_scroll,
                        theme,
                        cx,
                    )))
                    .children(edges)
                    .children(worktree_away)
                    .when(away, |graph| {
                        graph.child(
                            graph_button("graph-latest", "Latest →", theme)
                                .right(px(10.))
                                .on_scroll_wheel(cx.listener(Self::scroll_graph))
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
                                .on_scroll_wheel(cx.listener(Self::scroll_graph))
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
