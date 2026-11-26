use super::*;

/// Whether a node's `px` size is a height: its parent is a column (a tab bar
/// above the panes), else it is a width (a sidebar beside them).
pub(super) fn node_is_tall(root: &Node, id: &str) -> bool {
    fn parent_axis(n: &Node, id: &str) -> Option<Axis> {
        for child in n.children() {
            if child.id == id {
                return n.axis();
            }
            if let Some(axis) = parent_axis(child, id) {
                return Some(axis);
            }
        }
        None
    }
    parent_axis(root, id) == Some(Axis::Column)
}

/// The visible children of `pane`'s parent and their current shares.
pub(super) fn visible_sibling_weights(root: &Node, pane: &str) -> Option<Vec<(String, f32)>> {
    let parent = root.parent_of(pane)?;
    let children = parent.children().iter().filter(|n| !n.hidden).map(|n| {
        let weight = match n.size {
            Some(Span::Weight { weight }) => weight,
            _ => 1.0,
        };
        (n.id.clone(), weight)
    }).collect::<Vec<_>>();
    (!children.is_empty()).then_some(children)
}

#[derive(Clone)]
enum SlideTarget {
    Weight { id: String, from: f32, to: f32 },
    Px { id: String, from: f32, to: f32 },
}

pub(super) struct ActiveSlide {
    id: u64,
    finish: Option<Box<dyn FnOnce(&mut AppWindow, &mut Context<AppWindow>)>>,
}

#[derive(Default)]
pub(super) struct Slides {
    pub(super) active: HashMap<String, ActiveSlide>,
    pub(super) next_id: u64,
    pub(super) saved_px: HashMap<String, f32>,
    pub(super) closing_panes: HashSet<String>,
    pub(super) pending_split: Option<String>,
}

impl AppWindow {
    /// How long a slide lasts, in seconds.
    const SLIDE_SECS: f32 = 0.16;
    /// A zoom moves every pane of the tab, so it takes a little longer.
    const ZOOM_SLIDE_SECS: f32 = 0.22;

    /// The share of `pane` in each container up to the tab root: (node id, current weight).
    fn share_chain(&self, pane: &str) -> Vec<(String, f32)> {
        let Some(c) = self.model.tab_of(pane) else { return Vec::new() };
        let mut chain = Vec::new();
        let mut id = pane.to_owned();
        while let Some(parent) = c.root.parent_of(&id) {
            let weight = match c.root.find(&id).and_then(|n| n.size) {
                Some(Span::Weight { weight }) => weight,
                Some(Span::Px { .. }) => 1.0,
                None => 1.0,
            };
            chain.push((id.clone(), weight));
            id = parent.id.clone();
        }
        chain
    }

    /// Keeps a slide's terminals out of resize until it ends, so each program hears one size.
    fn lock_panes(&mut self, ids: &[String], freeze: &HashSet<String>, cx: &mut Context<Self>) {
        for id in ids {
            if let Some(t) = self.terms.get(id) {
                let view = t.view.clone();
                let frozen = freeze.contains(id);
                view.update(cx, |view, cx| {
                    view.lock_resize(frozen);
                    cx.notify();
                });
            }
        }
    }

    fn unlock_panes(&mut self, ids: &[String], forget: &HashSet<String>, cx: &mut Context<Self>) {
        for id in ids {
            if let Some(t) = self.terms.get(id) {
                let view = t.view.clone();
                let should_forget = forget.contains(id);
                view.update(cx, |view, cx| {
                    view.unlock_resize(should_forget);
                    cx.notify();
                });
            }
        }
    }

    /// Settles the terminals whose final sizes are known before a slide starts,
    /// and returns the held panes whose final size was not known.
    pub(super) fn pre_settle(
        &self,
        targets: &[String],
        sizes: &[(String, (u16, u16))],
        window: &Window,
        cx: &mut Context<Self>,
    ) -> HashSet<String> {
        let cell = crate::terminal::cell_size(window);
        let scale = window.scale_factor();
        let mut settled = HashSet::new();
        for (id, (cols, rows)) in sizes {
            if let Some(t) = self.terms.get(id) {
                t.view.update(cx, |view, _| view.settle(*cols, *rows, cell, scale));
                settled.insert(id.clone());
            }
        }
        targets.iter().filter(|id| !settled.contains(*id)).cloned().collect()
    }

    pub(super) fn finish_slide(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(mut slide) = self.slides.active.remove(key) else { return false };
        if let Some(finish) = slide.finish.take() {
            finish(self, cx);
        }
        true
    }

    pub(super) fn finish_all_slides(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<String> = self.slides.active.keys().cloned().collect();
        for key in keys {
            self.finish_slide(&key, cx);
        }
    }

    pub(super) fn has_chrome_slide(&self) -> bool {
        self.slides.active.keys().any(|key| key.starts_with("chrome:"))
    }

    #[allow(clippy::too_many_arguments)]
    fn start_slide(
        &mut self,
        key: String,
        targets: Vec<SlideTarget>,
        held_ids: Vec<String>,
        freeze: HashSet<String>,
        forget: HashSet<String>,
        secs: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_done: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        self.finish_slide(&key, cx);
        self.lock_panes(&held_ids, &freeze, cx);
        let slide_id = self.slides.next_id;
        self.slides.next_id += 1;
        let finish_targets = targets.clone();
        let finish_held = held_ids.clone();
        let finish_forget = forget.clone();
        let finish = move |this: &mut AppWindow, cx: &mut Context<AppWindow>| {
            for target in finish_targets {
                match target {
                    SlideTarget::Weight { id, to, .. } => {
                        if let Some(n) = this.find_node_mut(&id) {
                            n.size = Some(Span::Weight { weight: to });
                        }
                    }
                    SlideTarget::Px { id, to, .. } => {
                        if let Some(n) = this.find_node_mut(&id) {
                            n.size = Some(Span::Px { px: to });
                        }
                    }
                }
            }
            on_done(this, cx);
            this.unlock_panes(&finish_held, &finish_forget, cx);
        };
        self.slides.active.insert(key.clone(), ActiveSlide { id: slide_id, finish: Some(Box::new(finish)) });
        let has_px = targets.iter().any(|t| matches!(t, SlideTarget::Px { .. }));
        cx.spawn_in(window, async move |this, cx| {
            let started = Instant::now();
            loop {
                let t = (started.elapsed().as_secs_f32() / secs).min(1.0);
                let w = 5.0;
                let k = (1.0 - (1.0 + w * t) * (-(w * t)).exp()) / (1.0 - (1.0 + w) * (-w).exp());
                let finished = t >= 1.0;
                let Ok(current) = this.update_in(cx, |this, window, cx| {
                    if !this.slides.active.get(&key).is_some_and(|s| s.id == slide_id) {
                        return false;
                    }
                    if finished {
                        this.finish_slide(&key, cx);
                        this.sync(window, cx);
                    } else {
                        for target in &targets {
                            match target {
                                SlideTarget::Weight { id, from, to } => {
                                    if let Some(n) = this.find_node_mut(id) {
                                        n.size = Some(Span::Weight { weight: from + (to - from) * k });
                                    }
                                }
                                SlideTarget::Px { id, from, to } => {
                                    if let Some(n) = this.find_node_mut(id) {
                                        n.size = Some(Span::Px { px: from + (to - from) * k });
                                    }
                                }
                            }
                        }
                        if has_px && mods::publish_layout(&this.chrome) {
                            this.refresh_snapshot_mods(cx);
                        }
                    }
                    cx.notify();
                    true
                }) else {
                    return;
                };
                if !current || finished {
                    return;
                }
                let (tx, rx) = async_channel::bounded::<()>(1);
                let tx_timer = tx.clone();
                cx.on_next_frame(move |_, _| {
                    let _ = tx.try_send(());
                });
                let bg = cx.background_executor().clone();
                let timer_bg = bg.clone();
                bg.spawn(async move {
                    timer_bg.timer(Duration::from_millis(32)).await;
                    let _ = tx_timer.try_send(());
                })
                .detach();
                if rx.recv().await.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Zooms over the slide: zooming in runs the split chain's shares toward one
    /// pane holding the tab and lands the model's zoom when the slide ends;
    /// unzooming lands it at once and slides the shares back. Focus lands at the
    /// start either way, so the header, the veil and focused_pane follow the
    /// pane that grows instead of the old one.
    pub(super) fn slide_zoom(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab_id) = self.model.tab_of(pane).map(|c| c.id.clone()) else { return };
        self.finish_all_slides(cx);
        let Some(tab) = self.model.tab(&tab_id).cloned() else { return };
        if tab.root.panes().len() < 2 {
            self.model.toggle_zoom(pane);
            return;
        }
        let unzooming = tab.zoomed.as_deref() == Some(pane);
        // Zooming focuses at once, though the model's zoom lands when the slide
        // ends: the header, the veil and focused_pane follow the pane that grows
        // from the first frame.
        if !unzooming && let Some(c) = self.model.tab_mut(&tab_id) {
            c.focus(pane);
        }
        let chain = self.share_chain(pane);
        let held_ids: Vec<String> = tab.root.panes().into_iter().map(|n| n.id.clone()).collect();
        let freeze: HashSet<String> = held_ids.iter().cloned().collect();
        if unzooming {
            self.model.toggle_zoom(pane);
            let Some(tab) = self.model.tab(&tab_id) else { return };
            let settled_sizes: Vec<_> = held_ids.iter().filter_map(|id| {
                self.shown_size(tab, id, window).map(|size| (id.clone(), size))
            }).collect();
            let moving: HashSet<String> = HashSet::from([pane.to_owned()]);
            let forget: HashSet<String> = self.pre_settle(&held_ids, &settled_sizes, window, cx)
                .into_iter().filter(|id| moving.contains(id)).collect();
            for (id, _) in &chain {
                if let Some(n) = self.find_node_mut(id) {
                    n.size = Some(Span::Weight { weight: 60.0 });
                }
            }
            let targets = chain.iter().map(|(id, weight)| SlideTarget::Weight {
                id: id.clone(),
                from: 60.0,
                to: *weight,
            }).collect();
            self.start_slide(tab_id, targets, held_ids, freeze, forget, Self::ZOOM_SLIDE_SECS, window, cx, |_, _| {});
        } else {
            let settled_sizes: Vec<_> = self.zoomed_size(&tab, pane, window).map(|size| vec![(pane.to_owned(), size)]).unwrap_or_default();
            let moving: HashSet<String> = HashSet::from([pane.to_owned()]);
            let forget: HashSet<String> = self.pre_settle(&[pane.to_owned()], &settled_sizes, window, cx)
                .into_iter().filter(|id| moving.contains(id)).collect();
            let restore = chain.clone();
            let targets = chain.iter().map(|(id, weight)| SlideTarget::Weight {
                id: id.clone(),
                from: *weight,
                to: 60.0,
            }).collect();
            let id = pane.to_owned();
            self.start_slide(tab_id, targets, held_ids, freeze, forget, Self::ZOOM_SLIDE_SECS, window, cx, move |this, _cx| {
                this.model.toggle_zoom(&id);
                for (node, weight) in &restore {
                    if let Some(n) = this.find_node_mut(node) {
                        n.size = Some(Span::Weight { weight: *weight });
                    }
                }
            });
        }
    }

    /// Removes a pane after its exit animation, ending its terminal.
    pub(super) fn close_pane_now(&mut self, pane: &str, cx: &mut Context<Self>) {
        let addr = self.model.address_of(pane);
        self.model.remove_pane_evenly(pane);
        self.slides.closing_panes.remove(pane);
        self.close_terminals(addr.into_iter().collect(), cx);
    }

    /// The slide a pane closes with: the panes beside it grow to even shares
    /// while its own shrinks to nothing, then it goes. `ended` says its program
    /// is already gone (nothing left to end). False when it has no container to
    /// slide in; the caller takes another route.
    pub(super) fn slide_close(&mut self, pane: &str, ended: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(tab_id) = self.model.tab_of(pane).map(|c| c.id.clone()) else { return false };
        self.finish_all_slides(cx);
        let Some(c) = self.model.tab(&tab_id) else { return false };
        let from = visible_sibling_weights(&c.root, pane).filter(|children| children.iter().any(|(id, _)| id == pane));
        let Some(from) = from else { return false };
        let held_ids: Vec<String> = c.root.panes().into_iter().map(|n| n.id.clone()).collect();
        let siblings: Vec<String> = from.iter().filter(|(id, _)| id != pane).map(|(id, _)| id.clone()).collect();
        let mut final_root = c.root.clone();
        let _ = final_root.remove(pane);
        if siblings.len() >= 2
            && let Some(first) = siblings.iter().find(|id| final_root.find(id).is_some())
        {
            crate::model::even_siblings(&mut final_root, first);
        }
        let survivors: Vec<String> = final_root.panes().into_iter().map(|n| n.id.clone()).collect();
        let settled_sizes: Vec<_> = survivors.iter().filter_map(|id| {
            self.initial_size(&final_root, id, window).map(|size| (id.clone(), size))
        }).collect();
        let freeze: HashSet<String> = held_ids.iter().cloned().collect();
        let forget = self.pre_settle(&survivors, &settled_sizes, window, cx);
        let targets = from.iter().map(|(id, weight)| SlideTarget::Weight {
            id: id.clone(),
            from: *weight,
            to: if id == pane { 0.0 } else { 1.0 },
        }).collect();
        let closing = pane.to_owned();
        self.slides.closing_panes.insert(closing.clone());
        self.start_slide(tab_id, targets, held_ids, freeze, forget, Self::SLIDE_SECS, window, cx, move |this, cx| {
            if ended {
                this.model.remove_pane_evenly(&closing);
                this.slides.closing_panes.remove(&closing);
            } else {
                this.close_pane_now(&closing, cx);
            }
        });
        true
    }

    /// A pane whose program ended (or that was parked) closes with the same
    /// slide a hand's close plays. A tab's last pane just goes: there is
    /// nothing beside it to slide.
    pub(super) fn close_pane_on_its_own(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.slides.closing_panes.contains(pane) {
            return;
        }
        if let Some(tab_id) = self.model.tab_of(pane).map(|c| c.id.clone()) {
            self.finish_slide(&tab_id, cx);
        }
        let alone = !self.model.tab_of(pane).is_some_and(|c| c.root.panes().len() > 1);
        if alone || !self.slide_close(pane, true, window, cx) {
            self.slides.closing_panes.remove(pane);
            self.model.remove_pane_evenly(pane);
        }
    }

    /// A fresh split slides in: its share grows from a sliver to the share the split gave it.
    pub(super) fn slide_split_in(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab_id) = self.model.tab_of(pane).map(|c| c.id.clone()) else { return };
        self.finish_all_slides(cx);
        let Some(tab) = self.model.tab(&tab_id).cloned() else { return };
        let held_ids: Vec<String> = tab.root.panes().into_iter().map(|n| n.id.clone()).collect();
        let freeze: HashSet<String> = held_ids.iter().cloned().collect();
        if let Some(mut from) = visible_sibling_weights(&tab.root, pane) {
            // A split inside a pane's own row takes half of its share. The
            // slide starts from the share it had, so it hands the new pane
            // its room from where it stood instead of first jumping to the
            // smaller share. (Beside a fixed-px pane nothing was halved.)
            if let Some(i) = from.iter().position(|(id, _)| id == pane)
                && i > 0
                && let Some(Span::Weight { weight: given }) = tab.root.find(pane).and_then(|n| n.size)
                && let Some(Span::Weight { .. }) = tab.root.find(&from[i - 1].0).and_then(|n| n.size)
            {
                let target = from[i - 1].0.clone();
                from[i - 1].1 += given;
                let weight = from[i - 1].1;
                if let Some(n) = self.find_node_mut(&target) {
                    n.size = Some(Span::Weight { weight });
                }
            }
            let mut final_root = tab.root.clone();
            crate::model::even_siblings(&mut final_root, pane);
            let settled_sizes: Vec<_> = held_ids.iter().filter_map(|id| {
                self.initial_size(&final_root, id, window).map(|size| (id.clone(), size))
            }).collect();
            let forget = self.pre_settle(&held_ids, &settled_sizes, window, cx);
            if let Some(n) = self.find_node_mut(pane) {
                n.size = Some(Span::Weight { weight: 0.02 });
            }
            if let Some((_, weight)) = from.iter_mut().find(|(id, _)| id == pane) {
                *weight = 0.02;
            }
            let targets = from.iter().map(|(id, weight)| SlideTarget::Weight {
                id: id.clone(),
                from: *weight,
                to: 1.0,
            }).collect();
            self.start_slide(tab_id, targets, held_ids, freeze, forget, Self::SLIDE_SECS, window, cx, |_, _| {});
            return;
        }
        let end = match tab.root.find(pane).and_then(|n| n.size) {
            Some(Span::Weight { weight }) => weight,
            Some(Span::Px { .. }) => return,
            None => 1.0,
        };
        let mut final_root = tab.root.clone();
        if let Some(n) = final_root.find_mut(pane) {
            n.size = Some(Span::Weight { weight: end });
        }
        let settled_sizes: Vec<_> = held_ids.iter().filter_map(|id| {
            self.initial_size(&final_root, id, window).map(|size| (id.clone(), size))
        }).collect();
        let forget = self.pre_settle(&held_ids, &settled_sizes, window, cx);
        let start = (end * 0.06).max(0.01);
        if let Some(n) = self.find_node_mut(pane) {
            n.size = Some(Span::Weight { weight: start });
        }
        let targets = vec![SlideTarget::Weight { id: pane.to_owned(), from: start, to: end }];
        self.start_slide(tab_id, targets, held_ids, freeze, forget, Self::SLIDE_SECS, window, cx, |_, _| {});
    }

    pub(super) fn slide_chrome_node(&mut self, id: &str, mode: &str, window: &mut Window, cx: &mut Context<Self>) {
        let key = format!("chrome:{id}");
        // A sidebar is as wide as its `px`; a tab bar below the title is as
        // tall. Reading the wrong side makes a slide interrupted midway start
        // from half a window and jump.
        let tall = node_is_tall(&self.chrome, id);
        let span = |b: Bounds<Pixels>| if tall { f32::from(b.size.height) } else { f32::from(b.size.width) };
        let cur_px = self.chrome.find(id).and_then(|n| {
            if n.hidden {
                self.bounds.borrow().get(id).map(|b| span(*b) / settings::zoom())
            } else {
                match n.size {
                    Some(Span::Px { px }) => Some(px),
                    _ => None,
                }
            }
        });
        let was_sliding = self.finish_slide(&key, cx);
        self.finish_all_slides(cx);
        let Some(n) = self.chrome.find(id) else { return self.fail(format!("no node {id:?} in the shown layout")) };
        let want_show = match mode {
            "layout.toggle" => n.hidden,
            "layout.show" => true,
            _ => false,
        };
        if !n.hidden == want_show {
            if was_sliding {
                self.sync(window, cx);
                cx.notify();
            }
            return;
        }
        let explicit_px = match n.size {
            Some(Span::Px { px }) if px > 1.0 => Some(px),
            _ => None,
        };
        let full_px = if !want_show {
            let px = explicit_px
                .or_else(|| self.bounds.borrow().get(id).map(|b| span(*b) / settings::zoom()))
                .or_else(|| self.slides.saved_px.get(id).copied())
                .unwrap_or(260.0);
            self.slides.saved_px.insert(id.to_owned(), px);
            px
        } else {
            explicit_px.or_else(|| self.slides.saved_px.get(id).copied()).unwrap_or(260.0)
        };
        let orig_size = n.size;
        let orig_min = n.min;
        let from = if was_sliding {
            cur_px.unwrap_or(if want_show { 0.0 } else { full_px })
        } else if want_show {
            0.0
        } else {
            full_px
        };
        let to = if want_show { full_px } else { 0.0 };
        let final_size = match orig_size {
            Some(Span::Px { .. }) => Some(Span::Px { px: full_px }),
            other => other,
        };
        let mut final_chrome = self.chrome.clone();
        if let Some(n) = final_chrome.find_mut(id) {
            n.hidden = !want_show;
            n.size = final_size;
            n.min = orig_min;
        }
        let held_ids: Vec<String> = self.model.active_tab().map(|tab| {
            tab.root.panes().into_iter().map(|n| n.id.clone()).collect()
        }).unwrap_or_default();
        // A slide that moves the panes by rows (a tab bar above them) holds
        // their grids until it lands: resizing first makes the program draw its
        // new screen under chrome that has not moved, and then draw again, so
        // one toggle reads as two renders. A sidebar changes the width, where
        // the rewrap has to be in place before the first frame or the text
        // visibly jumps as it slides.
        let freeze: HashSet<String> = held_ids.iter().cloned().collect();
        let forget: HashSet<String> = if tall {
            HashSet::new()
        } else {
            let mut settled_sizes = Vec::new();
            if let Some(target_area) = self.context_area_for(&final_chrome)
                && let Some(tab) = self.model.active_tab()
            {
                let focused: HashSet<String> = tab.focused_pane().and_then(|f| tab.root.path_to(&f)).unwrap_or_default().into_iter().collect();
                let shown = tab.shown();
                for pane in shown.panes() {
                    if let Some(size) = self.size_in(shown, tab.root.panes().len(), &pane.id, target_area, &focused, window) {
                        settled_sizes.push((pane.id.clone(), size));
                    }
                }
            }
            self.pre_settle(&held_ids, &settled_sizes, window, cx)
        };
        if let Some(n) = self.chrome.find_mut(id) {
            if want_show {
                n.hidden = false;
            }
            n.min = None;
            n.size = Some(Span::Px { px: from });
        }
        let node_id = id.to_owned();
        let targets = vec![SlideTarget::Px { id: id.to_owned(), from, to }];
        self.start_slide(key, targets, held_ids, freeze, forget, Self::SLIDE_SECS, window, cx, move |this, _cx| {
            if let Some(n) = this.chrome.find_mut(&node_id) {
                n.min = orig_min;
                n.size = orig_size;
                n.hidden = !want_show;
            }
        });
        if want_show {
            self.sync(window, cx);
        } else if mods::publish_layout(&self.chrome) {
            self.refresh_snapshot_mods(cx);
        }
    }
}
