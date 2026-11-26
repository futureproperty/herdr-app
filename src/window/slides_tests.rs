#[test]
fn a_node_in_a_column_is_sized_by_its_height() {
    let tree: super::Node = serde_json::from_value(serde_json::json!({
        "id": "root", "type": "row", "children": [
            {"id": "sidebar", "type": "block", "content": {"mod": "a"}},
            {"id": "work", "type": "column", "children": [
                {"id": "tabbar", "type": "block", "content": {"mod": "b"}},
                {"id": "panes", "type": "row", "children": [
                    {"id": "inset", "type": "block", "content": {"mod": "c"}}
                ]}
            ]}
        ]
    })).unwrap();
    assert!(super::slides::node_is_tall(&tree, "tabbar"), "a tab bar's px is its height");
    assert!(!super::slides::node_is_tall(&tree, "sidebar"), "a sidebar's px is its width");
    assert!(!super::slides::node_is_tall(&tree, "inset"), "and a column's row child is wide again");
    assert!(!super::slides::node_is_tall(&tree, "nowhere"));
}

#[test]
fn slide_chrome_is_idempotent_and_preemptible() {
    in_own_process("slide_chrome_idempotent");
}

#[gpui::test]
fn slide_chrome_idempotent(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("slide_chrome_idempotent") { return; }
    let layout = json!({
        "id": "root", "type": "row", "children": [
            {"id": "sidebar", "type": "block", "size": {"px": 268}, "min": 44,
             "content": {"mod": "size-probe"}}
        ]
    });
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", layout);
    view.update_in(cx, |this, window, cx| {
        let slide = |this: &mut super::AppWindow, name: &str, args: serde_json::Value, window: &mut Window, cx: &mut Context<super::AppWindow>| {
            let reply = this.run_as(name, args, false, window, cx);
            assert!(reply.get("error").is_none(), "{reply}");
        };
        let sidebar = |this: &super::AppWindow| {
            let n = this.chrome.find("sidebar").expect("sidebar");
            (n.hidden, n.min, n.size, this.slides.active.contains_key("chrome:sidebar"))
        };

        slide(this, "layout.hide", json!({"id": "sidebar", "slide": true}), window, cx);
        assert_eq!(sidebar(this), (false, None, Some(super::Span::Px { px: 268.0 }), true));
        slide(this, "layout.hide", json!({"id": "sidebar", "slide": true}), window, cx);
        assert_eq!(sidebar(this), (true, Some(44.0), Some(super::Span::Px { px: 268.0 }), false), "a repeated hide must finish, not start a phantom slide");

        slide(this, "layout.show", json!({"id": "sidebar", "slide": true}), window, cx);
        assert_eq!(sidebar(this), (false, None, Some(super::Span::Px { px: 0.0 }), true));
        slide(this, "layout.show", json!({"id": "sidebar", "slide": true}), window, cx);
        assert_eq!(sidebar(this), (false, Some(44.0), Some(super::Span::Px { px: 268.0 }), false), "a repeated show must finish visibly");

        slide(this, "layout.hide", json!({"id": "sidebar", "slide": true}), window, cx);
        assert!(sidebar(this).3);
        slide(this, "layout.show", json!({"id": "sidebar"}), window, cx);
        assert_eq!(sidebar(this), (false, Some(44.0), Some(super::Span::Px { px: 268.0 }), false), "a non-slide command must finish the chrome slide");
    });
    cx.executor().advance_clock(std::time::Duration::from_millis(64));
    cx.run_until_parked();
}

#[test]
fn split_slide_starts_from_the_pane_s_old_share() {
    in_own_process("split_slide_share");
}

/// Splitting a pane halves its share in its row; the slide has to start
/// that pane from the share it had, so its edge moves from where it stood
/// instead of jumping to the halved share and sliding back.
#[gpui::test]
fn split_slide_share(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("split_slide_share") {
        return;
    }
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", json!({
        "id": "root", "type": "row", "children": [
            {"id": "context", "type": "block", "content": {"tab": "active"}}
        ]
    }));
    view.update_in(cx, |this, window, cx| {
        let weight = |this: &super::AppWindow, tab: &str, id: &str| match this.model.tab(tab).unwrap().root.find(id).and_then(|n| n.size) {
            Some(super::Span::Weight { weight }) => weight,
            other => panic!("{id} has no weight: {other:?}"),
        };
        // Two panes side by side (50/50), split the right one again. The
        // split halves it to 25%, but the slide starts it from the 50% it
        // had, so its left edge moves from the middle instead of 2/3.
        this.model.add_space("s1", "one", "", super::Tab::new("t", "t", "pa", "ta"));
        assert!(this.model.split("pa", crate::layout::Axis::Row, "pb", "tb"));
        assert!(this.model.split("pb", crate::layout::Axis::Row, "pc", "tc"));
        this.model.select_tab("t");
        this.sync(window, cx);
        this.slide_split_in("pc", window, cx);
        assert_eq!(weight(this, "t", "pb"), 0.5, "the split pane starts from the share it had");
        assert_eq!(weight(this, "t", "pa"), 0.5, "its sibling is untouched");
        assert!(weight(this, "t", "pc") < 0.1, "the new pane starts as a sliver");
    });
    // One pane above another, split the lower one sideways: it wraps, and
    // the wrapped pane keeps the share it had.
    view.update_in(cx, |this, window, cx| {
        this.model.add_space("s2", "two", "", super::Tab::new("u", "u", "qa", "ta"));
        assert!(this.model.split("qa", crate::layout::Axis::Column, "qb", "tb"));
        assert!(this.model.split("qb", crate::layout::Axis::Row, "qc", "tc"));
        this.model.select_tab("u");
        this.sync(window, cx);
        this.slide_split_in("qc", window, cx);
        assert!(this.model.tab("u").unwrap().root.find("qb").unwrap().size.is_none(), "a wrapped pane keeps its own share");
        assert!(matches!(this.model.tab("u").unwrap().root.find("qc").unwrap().size, Some(super::Span::Weight { weight }) if weight < 0.1));
    });
}

#[test]
fn even_siblings_evens_just_the_split_pane_s_row() {
    let pane = |id: &str, weight: f32| {
        let mut n = super::Node::block(id, super::Content::Terminal { terminal: String::new() });
        n.size = Some(super::Span::Weight { weight });
        n
    };
    let mut hidden = pane("pc", 0.25);
    hidden.hidden = true;
    let mut root = super::Node::container("root", crate::layout::Axis::Column, vec![
        super::Node::container("row", crate::layout::Axis::Row, vec![pane("pa", 1.0), pane("pb", 0.5), pane("pc2", 0.25), hidden]),
        pane("pd", 2.0),
    ]);
    crate::model::even_siblings(&mut root, "pc2");
    let weight = |id: &str| root.find(id).and_then(|n| n.size);
    for id in ["pa", "pb", "pc2"] {
        assert_eq!(weight(id), Some(super::Span::Weight { weight: 1.0 }), "{id} evens out");
    }
    assert_eq!(weight("pc"), Some(super::Span::Weight { weight: 0.25 }), "a hidden sibling keeps its share");
    assert_eq!(weight("pd"), Some(super::Span::Weight { weight: 2.0 }), "another row is left alone");
}

#[test]
fn zoomed_size_keeps_the_pane_header() {
    in_own_process("zoomed_size_header");
}

#[gpui::test]
fn zoomed_size_header(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("zoomed_size_header") { return; }
    let framed = json!({"id": "context", "type": "block", "content": {"tab": "active", "pane": {
        "lone": false,
        "header": {"height": 80, "content": {"mod": "size-probe"}}
    }}});
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", framed);
    view.update_in(cx, |this, window, cx| {
        this.folders.retain(|id, _| id == "size-probe");
        this.model = crate::model::Model::default();
        let mut tab = super::Tab::new("tab", "", "a", "probe-a");
        tab.root.split("a", super::Axis::Row, crate::model::pane("b", "probe-b"), "split".into());
        tab.focus("a");
        this.model.add_space("space", "test", "/", tab);
        this.sync(window, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (zoomed, without_header) = view.update_in(cx, |this, window, _| {
        let tab = this.model.tab("tab").unwrap();
        let zoomed = this.zoomed_size(tab, "a", window).expect("a zoomed grid");
        let pane = tab.root.find("a").unwrap();
        let without_header = this.initial_size(pane, "a", window).expect("an unframed grid");
        (zoomed, without_header)
    });
    assert_eq!(zoomed.0, without_header.0, "the fixed pane header only changes the grid's height");
    assert!(zoomed.1 < without_header.1, "lone:false with more than one pane must reserve the header: {zoomed:?} vs {without_header:?}");

    view.update_in(cx, |this, window, cx| {
        this.model.tab_mut("tab").unwrap().zoomed = Some("a".into());
        this.sync(window, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let actual = view.update(cx, |this, cx| this.terms["a"].view.read(cx).dimensions());
    assert_eq!(actual, zoomed, "the zoomed terminal must be fit to the framed grid");
}

#[test]
fn chrome_slide_pre_settles_a_sidebar() {
    in_own_process("chrome_slide_pre_settle");
}

/// A sidebar changes the width, so the pane is settled to the area it will
/// have before the first animation frame: the text is already rewrapped as
/// the columns say, and slides without a jump.
#[gpui::test]
fn chrome_slide_pre_settle(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("chrome_slide_pre_settle") { return; }
    let layout = json!({
        "id": "root", "type": "row", "children": [
            {"id": "sidebar", "type": "block", "size": {"px": 268}, "min": 44,
             "content": {"mod": "size-probe"}},
            {"id": "work", "type": "column", "children": [
                {"id": "context", "type": "block", "content": {"tab": "active"}}
            ]}
        ]
    });
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", layout);
    view.update_in(cx, |this, window, cx| {
        this.model = crate::model::Model::default();
        this.model.add_space("space", "test", "/", super::Tab::new("tab", "", "a", "probe-a"));
        this.sync(window, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (visible_size, hidden_size) = view.update_in(cx, |this, window, _| {
        let area = |hidden| {
            let mut chrome = this.chrome.clone();
            chrome.find_mut("sidebar").unwrap().hidden = hidden;
            this.context_area_for(&chrome).expect("context area")
        };
        let (visible_area, hidden_area) = (area(false), area(true));
        let tab = this.model.active_tab().unwrap();
        let focused: std::collections::HashSet<String> = tab.focused_pane().and_then(|f| tab.root.path_to(&f)).unwrap_or_default().into_iter().collect();
        let shown = tab.shown();
        let visible_size = this.size_in(shown, tab.root.panes().len(), "a", visible_area, &focused, window).unwrap();
        let hidden_size = this.size_in(shown, tab.root.panes().len(), "a", hidden_area, &focused, window).unwrap();
        assert_ne!(visible_size, hidden_size);
        (visible_size, hidden_size)
    });
    let dims = |cx: &mut gpui::TestAppContext| view.update(cx, |this, cx| this.terms["a"].view.read(cx).dimensions());
    assert_eq!(dims(cx), visible_size, "the drawn pane starts at its visible size");

    view.update_in(cx, |this, window, cx| {
        let out = this.run_as("layout.hide", json!({"id": "sidebar", "slide": true}), false, window, cx);
        assert!(out.get("error").is_none(), "{out}");
        assert!(this.has_chrome_slide());
        assert_eq!(this.terms["a"].view.read(cx).dimensions(), hidden_size, "hide settles the pane before its first animation frame");
    });
    view.update_in(cx, |this, _window, cx| this.finish_all_slides(cx));
    view.update_in(cx, |this, window, cx| {
        let out = this.run_as("layout.show", json!({"id": "sidebar", "slide": true}), false, window, cx);
        assert!(out.get("error").is_none(), "{out}");
        assert!(this.has_chrome_slide());
        assert_eq!(this.terms["a"].view.read(cx).dimensions(), visible_size, "show settles it before its first animation frame");
    });
}

#[test]
fn chrome_slide_settles_a_tab_bar_when_it_lands() {
    in_own_process("chrome_slide_tab_bar");
}

/// A tab bar takes rows from the panes: their grids are left alone while it
/// slides and resized once when it lands, so the program redraws once
/// instead of drawing under chrome that has not moved and drawing again.
#[gpui::test]
fn chrome_slide_tab_bar(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("chrome_slide_tab_bar") { return; }
    let layout = json!({
        "id": "root", "type": "column", "children": [
            {"id": "tabbar", "type": "block", "size": {"px": 30}, "content": {"mod": "size-probe"}},
            {"id": "work", "type": "block", "content": {"tab": "active"}}
        ]
    });
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", layout);
    view.update_in(cx, |this, window, cx| {
        this.model = crate::model::Model::default();
        this.model.add_space("space", "test", "/", super::Tab::new("tab", "", "a", "probe-a"));
        this.sync(window, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (visible_size, hidden_size) = view.update_in(cx, |this, window, _| {
        let area = |hidden| {
            let mut chrome = this.chrome.clone();
            chrome.find_mut("tabbar").unwrap().hidden = hidden;
            this.context_area_for(&chrome).expect("context area")
        };
        let (visible_area, hidden_area) = (area(false), area(true));
        assert!(hidden_area.height > visible_area.height, "hiding the tab bar must make the tab taller");
        let tab = this.model.active_tab().unwrap();
        let focused: std::collections::HashSet<String> = tab.focused_pane().and_then(|f| tab.root.path_to(&f)).unwrap_or_default().into_iter().collect();
        let shown = tab.shown();
        let visible_size = this.size_in(shown, tab.root.panes().len(), "a", visible_area, &focused, window).unwrap();
        let hidden_size = this.size_in(shown, tab.root.panes().len(), "a", hidden_area, &focused, window).unwrap();
        assert_ne!(visible_size, hidden_size);
        (visible_size, hidden_size)
    });
    let dims = |cx: &mut gpui::TestAppContext| view.update(cx, |this, cx| this.terms["a"].view.read(cx).dimensions());
    assert_eq!(dims(cx), visible_size, "the drawn pane starts at its visible size");

    view.update_in(cx, |this, window, cx| {
        let out = this.run_as("layout.hide", json!({"id": "tabbar", "slide": true}), false, window, cx);
        assert!(out.get("error").is_none(), "{out}");
        assert!(this.has_chrome_slide());
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(dims(cx), visible_size, "a paused slide frame keeps the pane's grid");
    view.update_in(cx, |this, _window, cx| this.finish_all_slides(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(dims(cx), hidden_size, "the grid lands with the slide");
}

#[test]
fn save_and_switch_layout_are_safe_during_a_chrome_slide() {
    in_own_process("chrome_slide_save");
}

#[gpui::test]
fn chrome_slide_save(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("chrome_slide_save") { return; }
    let layout = json!({
        "id": "root", "type": "row", "children": [
            {"id": "sidebar", "type": "block", "size": {"px": 268}, "min": 44,
             "content": {"mod": "size-probe"}},
            {"id": "context", "type": "block", "content": {"tab": "active"}}
        ]
    });
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", layout);
    std::fs::write(super::layout_path("other"), json!({
        "id": "other", "type": "block", "content": {"mod": "size-probe"}
    }).to_string()).unwrap();
    view.update_in(cx, |this, window, cx| {
        let layout_path = super::layout_path("test");
        let state_path = super::state_path("test");
        let layout_before = std::fs::read_to_string(&layout_path).unwrap();
        let state_before = std::fs::read_to_string(&state_path).ok();
        let out = this.run_as("layout.hide", json!({"id": "sidebar", "slide": true}), false, window, cx);
        assert!(out.get("error").is_none(), "{out}");
        assert!(this.has_chrome_slide());
        this.chrome.find_mut("sidebar").unwrap().size = Some(super::Span::Px { px: 123.0 });
        this.save();
        assert_eq!(std::fs::read_to_string(&layout_path).unwrap(), layout_before, "save must not write layout during a chrome slide");
        assert_eq!(std::fs::read_to_string(&state_path).ok(), state_before, "save must not write state during a chrome slide");

        this.switch_layout(Some("other"), window, cx);
        assert!(this.slides.active.is_empty(), "switch_layout must finish every slide");
        assert_eq!(this.layout_name, "other");
        assert!(this.slides.saved_px.is_empty(), "switching layouts clears saved chrome widths");
    });
}
