// app/audiolib-desktop/src/tag_chips.rs
//! Include/exclude tag terms on the sidebar filter page. `tag-filter` holds the canonical
//! term text (`audiolib_core::tag_filter`); this module edits it and wraps the chips into
//! rows, because the Slint layouts available here cannot wrap a repeated row by width.
use crate::{AppWindow, TagChip, TagChipRow};
use audiolib_core::{
    MAX_TAG_TERMS, SearchFilters,
    tag_filter::{self, TagTerm},
};
use slint::{Color, ModelRc, VecModel};

/// The terms currently held by the filter page.
pub(crate) fn terms(ui: &AppWindow) -> Vec<TagTerm> {
    tag_filter::parse_tag_terms(&ui.get_tag_filter())
}

/// Store the page's terms in the tag fields of a search request.
pub(crate) fn apply_to(ui: &AppWindow, filters: &mut SearchFilters) {
    tag_filter::apply_tag_terms(filters, &terms(ui));
}

fn store(ui: &AppWindow, terms: &[TagTerm]) {
    ui.set_tag_filter(tag_filter::format_tag_terms(terms).into());
    relayout(ui);
}

/// Add the typed draft to the terms. A draft holding no term (for example a lone sign)
/// stays in the field so nothing typed is silently discarded. A draft that would exceed
/// [`MAX_TAG_TERMS`] also stays, and the terms are unchanged: false, so the caller can
/// say why instead of showing chips no search accepts.
pub(crate) fn commit_draft(ui: &AppWindow) -> bool {
    let added = tag_filter::parse_tag_terms(&ui.get_tag_draft());
    if added.is_empty() {
        return true;
    }
    let mut terms = terms(ui);
    tag_filter::merge_tag_terms(&mut terms, added);
    if terms.len() > MAX_TAG_TERMS {
        return false;
    }
    ui.set_tag_draft("".into());
    store(ui, &terms);
    true
}

/// Switch a term between include and exclude.
pub(crate) fn toggle(ui: &AppWindow, index: i32) {
    let mut terms = terms(ui);
    if let Some(term) = usize::try_from(index)
        .ok()
        .and_then(|index| terms.get_mut(index))
    {
        term.exclude = !term.exclude;
        store(ui, &terms);
    }
}

/// Remove a term; a negative index removes the last one.
pub(crate) fn remove(ui: &AppWindow, index: i32) {
    let mut terms = terms(ui);
    let index = usize::try_from(index)
        .ok()
        .or_else(|| terms.len().checked_sub(1));
    if let Some(index) = index.filter(|index| *index < terms.len()) {
        terms.remove(index);
        store(ui, &terms);
    }
}

/// Rebuild the chip rows: greedy line filling with chips measured by the hidden probe,
/// so wrapping follows the real font, interface scale and sidebar width.
pub(crate) fn relayout(ui: &AppWindow) {
    let terms = terms(ui);
    ui.set_tag_term_count(terms.len() as i32);
    let area = ui.get_tag_chip_area_width();
    let spacing = ui.get_tag_chip_spacing();
    let mut rows: Vec<Vec<TagChip>> = Vec::new();
    let mut used = 0.0;
    for (index, term) in terms.iter().enumerate() {
        ui.set_tag_probe_text(term.text.as_str().into());
        let width = ui.get_tag_probe_width();
        let chip = TagChip {
            text: term.text.as_str().into(),
            exclude: term.exclude,
            index: index as i32,
        };
        // Keep a pixel in reserve so rounding can never push a chip past the edge.
        match rows.last_mut() {
            Some(row) if used + spacing + width <= area - 1.0 => {
                used += spacing + width;
                row.push(chip);
            }
            _ => {
                used = width;
                rows.push(vec![chip]);
            }
        }
    }
    ui.set_tag_probe_text("".into());
    let rows: Vec<TagChipRow> = rows
        .into_iter()
        .map(|chips| TagChipRow {
            chips: ModelRc::new(VecModel::from(chips)),
        })
        .collect();
    ui.set_tag_chip_rows(ModelRc::new(VecModel::from(rows)));
}

/// Show the typed `#RRGGBB` beside the color field; anything else leaves the swatch empty.
pub(crate) fn update_color_swatch(ui: &AppWindow) {
    let text = ui.get_color_filter();
    let hex = text.trim();
    let value = hex
        .strip_prefix('#')
        .filter(|digits| digits.len() == 6)
        .and_then(|digits| u32::from_str_radix(digits, 16).ok());
    ui.set_color_filter_valid(value.is_some());
    if let Some(value) = value {
        ui.set_color_filter_swatch(Color::from_argb_encoded(0xff00_0000 | value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::{ComponentHandle, Model};

    fn chips(ui: &AppWindow) -> Vec<Vec<(String, bool, i32)>> {
        let rows = ui.get_tag_chip_rows();
        (0..rows.row_count())
            .map(|row| {
                let chips = rows.row_data(row).unwrap().chips;
                (0..chips.row_count())
                    .map(|index| {
                        let chip = chips.row_data(index).unwrap();
                        (chip.text.to_string(), chip.exclude, chip.index)
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn draft_terms_become_chips_that_toggle_remove_and_wrap() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        ui.set_tag_chip_area_width(10_000.0);
        ui.set_tag_draft("engine -interior, \"pass by\"".into());
        commit_draft(&ui);
        assert_eq!(ui.get_tag_draft(), "");
        assert_eq!(ui.get_tag_filter(), "engine, -interior, \"pass by\"");
        assert_eq!(ui.get_tag_term_count(), 3);
        assert_eq!(
            chips(&ui),
            [vec![
                ("engine".into(), false, 0),
                ("interior".into(), true, 1),
                ("pass by".into(), false, 2)
            ]]
        );
        // Entering a term again with the other sign switches it in place.
        ui.set_tag_draft("-ENGINE".into());
        commit_draft(&ui);
        assert_eq!(ui.get_tag_filter(), "-engine, -interior, \"pass by\"");
        toggle(&ui, 1);
        remove(&ui, -1);
        assert_eq!(ui.get_tag_filter(), "-engine, interior");
        let mut filters = SearchFilters::default();
        apply_to(&ui, &mut filters);
        assert_eq!(filters.tag_contains.as_deref(), Some("interior"));
        assert_eq!(filters.exclude_tags, ["engine"]);
        // A lone sign is not a term and stays in the field.
        ui.set_tag_draft("-".into());
        assert!(commit_draft(&ui));
        assert_eq!(ui.get_tag_draft(), "-");
        // Terms past the search limit stay in the field instead of becoming chips.
        let many = (0..MAX_TAG_TERMS)
            .map(|index| format!("t{index}"))
            .collect::<Vec<_>>();
        ui.set_tag_draft(many.join(" ").into());
        assert!(!commit_draft(&ui));
        assert_eq!(ui.get_tag_filter(), "-engine, interior");
        // Re-entering an existing term with the other sign does not add one.
        ui.set_tag_draft("engine".into());
        assert!(commit_draft(&ui));
        ui.set_tag_draft("-engine".into());
        assert!(commit_draft(&ui));
        assert_eq!(ui.get_tag_filter(), "-engine, interior");
        // A narrow area puts each chip on its own row; indexes keep the term order.
        let one_chip = {
            ui.set_tag_probe_text("interior".into());
            ui.get_tag_probe_width()
        };
        ui.set_tag_chip_area_width(one_chip + 2.0);
        relayout(&ui);
        assert_eq!(
            chips(&ui),
            [
                vec![("engine".into(), true, 0)],
                vec![("interior".into(), false, 1)]
            ]
        );
        remove(&ui, 0);
        remove(&ui, 0);
        remove(&ui, 0);
        assert_eq!(ui.get_tag_filter(), "");
        assert!(chips(&ui).is_empty());
        assert_eq!(ui.get_tag_term_count(), 0);
    }

    fn click_labelled(ui: &AppWindow, label: &str) {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::{PointerEventButton, WindowEvent};
        let element = ElementHandle::find_by_accessible_label(ui, label)
            .next()
            .unwrap_or_else(|| panic!("Missing {label}"));
        let origin = element.absolute_position();
        let size = element.size();
        let position =
            slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        ui.window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        ui.window().dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        ui.window().dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    }

    fn type_text(ui: &AppWindow, text: &str) {
        for character in text.chars() {
            crate::ui_tests::send_keys(ui, &[character.to_string().into()]);
        }
    }

    #[test]
    fn enter_commits_terms_and_backspace_or_chip_buttons_edit_them() {
        use slint::platform::Key;
        use std::{cell::RefCell, rc::Rc};
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        ui.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        ui.set_filters_visible(true);
        let events = Rc::new(RefCell::new(Vec::new()));
        let weak = ui.as_weak();
        let observed = events.clone();
        ui.on_apply_filters(move || {
            commit_draft(&weak.unwrap());
            observed.borrow_mut().push("apply".to_owned());
        });
        let weak = ui.as_weak();
        let observed = events.clone();
        ui.on_tag_term_removed(move |index| {
            remove(&weak.unwrap(), index);
            observed.borrow_mut().push(format!("remove {index}"));
        });
        let weak = ui.as_weak();
        let observed = events.clone();
        ui.on_tag_term_toggled(move |index| {
            toggle(&weak.unwrap(), index);
            observed.borrow_mut().push(format!("toggle {index}"));
        });
        let weak = ui.as_weak();
        ui.on_tag_chips_relayout(move || relayout(&weak.unwrap()));
        ui.show().unwrap();
        click_labelled(&ui, "Tag filter: + include, - exclude, Enter adds");
        type_text(&ui, "engine -rain door");
        crate::ui_tests::send_keys(&ui, &[Key::Return.into()]);
        assert_eq!(ui.get_tag_filter(), "engine, -rain, door");
        assert_eq!(ui.get_tag_draft(), "");
        // Backspace edits typed text first and removes the last term only when empty.
        type_text(&ui, "x");
        crate::ui_tests::send_keys(&ui, &[Key::Backspace.into()]);
        assert_eq!(ui.get_tag_filter(), "engine, -rain, door");
        crate::ui_tests::send_keys(&ui, &[Key::Backspace.into()]);
        assert_eq!(ui.get_tag_filter(), "engine, -rain");
        click_labelled(&ui, "Include rain");
        assert_eq!(ui.get_tag_filter(), "engine, rain");
        click_labelled(&ui, "Remove tag engine");
        assert_eq!(ui.get_tag_filter(), "rain");
        assert_eq!(
            &*events.borrow(),
            &["apply", "remove -1", "toggle 1", "remove 0"]
        );
        ui.hide().unwrap();
    }

    #[test]
    fn color_swatch_follows_valid_hex_only() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        ui.set_color_filter(" #3A7Bd5 ".into());
        update_color_swatch(&ui);
        assert!(ui.get_color_filter_valid());
        assert_eq!(
            ui.get_color_filter_swatch(),
            Color::from_rgb_u8(0x3a, 0x7b, 0xd5)
        );
        for invalid in ["", "#12345", "red", "#GGGGGG"] {
            ui.set_color_filter(invalid.into());
            update_color_swatch(&ui);
            assert!(!ui.get_color_filter_valid(), "{invalid}");
        }
    }
}
