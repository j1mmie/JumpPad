use iced::widget::column;

use super::styles::{
    drop_overlay_style, find_button_style, find_palette_style,
    find_input_style, modal_choice, modal_dialog, modal_scrim_style,
    new_tab_style, round_icon_button, tab_bar_style, tab_close_style,
    tab_frame_style, tab_title_style,
};
use super::*;

impl JumpPadApp {
    /// The widget a window should hand keyboard focus to, by id, or `None`
    /// when nothing on screen wants it.
    pub(super) fn focus_target(&self) -> Option<&'static str> {
        if self.modal.is_some() {
            // A modal's choices are driven from the app's own key handling
            // and its focused choice is a field of the modal, so there is no
            // widget here to hand anything to.
            return None;
        }

        match self.active_find().filter(|find| find.open) {
            Some(_) => Some(FIND_INPUT_ID),
            None => Some(editor_core::EDITOR_WIDGET_ID),
        }
    }

    /// Puts keyboard focus where [`focus_target`](Self::focus_target) says.
    ///
    /// Focus belongs to a window's own widget tree, so a window that has
    /// just appeared has none of it, however much of the app's state carried
    /// into it - the caret and the selection do, since those live in the
    /// document rather than in the widgets drawing it.
    ///
    /// Not `focus_find` for the palette, though the id is the same: that one
    /// selects the query as well, which is right when reopening the palette
    /// and wrong here, where the next keystroke would wipe a query the user
    /// was midway through.
    pub(super) fn restore_focus(&self) -> Task<Message> {
        match self.focus_target() {
            Some(id) => operate(operation::focusable::focus(Id::new(id))),
            None => Task::none(),
        }
    }

    /// The bar shown over the active tab when its file changed on disk while
    /// it had unsaved edits - VS Code shows one for the same reason, since
    /// the conflict otherwise stays invisible until the next save.
    fn changed_on_disk_bar(&self, tab_id: u64) -> Element<'_, Message> {
        let ui = self.ui_text;
        let action = |label: &'static str, message: Message| {
            button(ui.control_text(label))
                .padding([4, 8])
                .style(find_button_style)
                .on_press(message)
        };

        container(
            row![
                ui.control_text("This file has changed on disk."),
                action("Reload", Message::ReloadFromDisk(tab_id)),
                action("Keep mine", Message::AcknowledgeExternalChange(tab_id)),
            ]
            .spacing(8)
            .align_y(Center),
        )
        .width(Fill)
        .padding(6)
        .style(find_palette_style)
        .into()
    }

    /// The find palette: query field, match counter, previous/next, close.
    fn find_palette(&self, state: &FindState) -> Element<'_, Message> {
        let ui = self.ui_text;
        let query = text_input("Find", &state.query)
            .id(Id::new(FIND_INPUT_ID))
            .on_input(Message::FindQueryChanged)
            .on_submit(Message::FindNext)
            .padding([4, 8])
            .font(ui.font)
            .size(ui.input_size())
            .width(Pixels(180.0))
            .style(find_input_style);

        let counter: Element<'_, Message> = match state.counter() {
            Some(label) => ui.control_text(label).into(),
            None => text("").into(),
        };

        let step = |content: Text<'static>, message: Message| {
            button(content)
                .padding([4, 6])
                .style(find_button_style)
                .on_press(message)
        };

        container(
            row![
                query,
                counter,
                step(ui.control_text("\u{2191}"), Message::FindPrevious),
                step(ui.control_text("\u{2193}"), Message::FindNext),
                step(
                    ui.control_icon(jumppad_icons::CLOSE),
                    Message::CloseFind,
                ),
            ]
            .spacing(6)
            .align_y(Center),
        )
        .padding(6)
        .style(find_palette_style)
        .into()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let ui = self.ui_text;
        let tab_chips = self.tabs.iter().enumerate().map(|(index, tab)| {
            let is_active = index == self.active;

            let title = button(ui.tab_text(tab.title()))
                .padding([TAB_VERTICAL_PADDING, 10.0])
                .style(move |theme, status| {
                    tab_title_style(theme, status, is_active)
                })
                .on_press(Message::SelectTab(index));

            let close_diameter = ui.close_button_diameter();
            let close = round_icon_button(
                ui.control_icon(jumppad_icons::CLOSE),
                close_diameter,
            )
            .style(move |theme, status| {
                tab_close_style(theme, status, is_active, close_diameter)
            })
            .on_press(Message::CloseTab(index));

            // The frame is the only thing that paints this tab's background -
            // title and close stay fully transparent so there's one seamless surface.
            let frame = container(row![title, close].align_y(Center))
                .padding(Padding::ZERO.right(8))
                .style(move |theme| tab_frame_style(theme, is_active));

            // Middle-click closes it too, same as the close button.
            mouse_area(frame)
                .on_middle_press(Message::CloseTab(index))
                .into()
        });

        let new_tab_diameter = ui.new_tab_button_diameter();
        let new_tab_button = container(
            round_icon_button(
                ui.tab_icon(jumppad_icons::ADD),
                new_tab_diameter,
            )
            .style(move |theme, status| {
                new_tab_style(theme, status, new_tab_diameter)
            })
            .on_press(Message::NewTab),
        )
        .padding(Padding::ZERO.left(6).right(6))
        .height(ui.strip_height())
        .align_y(Center)
        .style(tab_bar_style);

        let tabs_row =
            row(tab_chips.chain(std::iter::once(new_tab_button.into())))
                .spacing(0)
                .align_y(Center);

        // No shared background container behind the row - on a transparent
        // window every extra layer compounds opacity, so the row is painted
        // once, in pieces. `filler` covers the leftover space past the last
        // chip, matching `title`'s padding and line height so its height lines
        // up without relying on flex cross-axis sizing.
        let filler = container(ui.tab_text(""))
            .padding([TAB_VERTICAL_PADDING, 10.0])
            .width(Fill)
            .style(tab_bar_style);

        // The scrollbar is hidden rather than absent: the row still scrolls
        // by wheel or trackpad, but a floating bar over a strip this short
        // sits across the tab titles and makes them unreadable.
        let tab_bar: Element<'_, Message> = row![
            scrollable(tabs_row).direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::hidden(),
            )),
            filler,
        ]
        .width(Fill)
        .align_y(Center)
        .into();

        let editor: Element<'_, Message> =
            if let Some(tab) = self.tabs.get(self.active) {
                let index = self.active;
                let tab_id = tab.id;
                let view = tab
                    .editor
                    .view()
                    .map(move |message| Message::Editor(index, message));
                // Keyed by the tab's stable id, not its Vec index, so switching
                // tabs replaces the editor widget instead of reusing stale state.
                keyed_column([(tab_id, view)])
                    .width(Fill)
                    .height(Fill)
                    .into()
            } else {
                ui.body_text("No open tabs").into()
            };

        // Composed before the find palette so the palette floats above the
        // bar rather than under it - the bar's controls sit at its left end,
        // where the top-right palette doesn't reach.
        let editor = match self
            .tabs
            .get(self.active)
            .filter(|tab| tab.externally_changed)
        {
            Some(tab) => stack![
                editor,
                container(self.changed_on_disk_bar(tab.id))
                    .width(Fill)
                    .height(Fill)
                    .align_y(Top)
            ]
            .into(),
            None => editor,
        };

        // Floated over the editor rather than the whole window, so it never
        // covers the tab bar. `stack!` is the same overlay the modal uses.
        let editor = match self.active_find().filter(|state| state.open) {
            Some(state) => stack![
                editor,
                container(self.find_palette(state))
                    .width(Fill)
                    .height(Fill)
                    .align_x(Right)
                    .align_y(Top)
                    .padding(8)
            ]
            .into(),
            None => editor,
        };

        // Same overlay treatment, and for the same reason: the tab bar stays
        // visible and clickable underneath a drag. A plain container captures
        // no events, so the drop itself still lands.
        let editor = if self.files_hovered {
            stack![
                editor,
                container(center(ui.body_text("Drop to open")))
                    .width(Fill)
                    .height(Fill)
                    .style(drop_overlay_style)
            ]
            .into()
        } else {
            editor
        };

        let mut content = column![tab_bar, editor];

        if let Some(error) = &self.error {
            content = content.push(
                row![
                    ui.body_text(error.clone())
                        .color(iced::Color::from_rgb8(220, 60, 60)),
                    button(ui.body_text("Dismiss"))
                        .on_press(Message::DismissError),
                ]
                .spacing(10)
                .padding(6),
            );
        }

        let Some(modal) = &self.modal else {
            return content.into();
        };

        let dialog = match modal {
            Modal::Close(pending) => {
                // Wired up with `on_press` too, so a click works the same as
                // Enter/Space.
                let choice =
                    |label: &'static str,
                     index: usize,
                     decision: CloseDecision| {
                        modal_choice(ui, label, pending.focused == index)
                            .on_press(Message::CloseConfirmed(
                                pending.tab_id,
                                decision,
                            ))
                    };
                modal_dialog(
                    ui,
                    format!(
                        "Do you want to save the changes you made to {}?",
                        pending.title
                    ),
                    row![
                        choice("Save", 0, CloseDecision::Save),
                        choice("Don't Save", 1, CloseDecision::DontSave),
                        choice("Cancel", 2, CloseDecision::Cancel),
                    ],
                )
            }
            Modal::SaveConflict(pending) => {
                let choice =
                    |label: &'static str,
                     index: usize,
                     decision: ConflictDecision| {
                        modal_choice(ui, label, pending.focused == index)
                            .on_press(Message::ConflictResolved(
                                pending.tab_id,
                                decision,
                            ))
                    };
                modal_dialog(
                    ui,
                    format!(
                        "{} has changed on disk since you opened it.",
                        pending.title
                    ),
                    row![
                        choice("Overwrite", 0, ConflictDecision::Overwrite),
                        choice(
                            "Discard & Reload",
                            1,
                            ConflictDecision::DiscardAndReload
                        ),
                        choice("Cancel", 2, ConflictDecision::Cancel),
                    ],
                )
            }
        };

        // Covers the window to block clicks reaching what's underneath; no
        // `on_press`, so clicking it can't lose unsaved work by accident.
        let scrim = mouse_area(
            container(text(""))
                .width(Fill)
                .height(Fill)
                .style(modal_scrim_style),
        );

        stack![content, scrim, center(dialog)].into()
    }
}
