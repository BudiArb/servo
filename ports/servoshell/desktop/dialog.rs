/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::path::Path;

use egui::{
    Area, Button, CornerRadius, Frame, Id, Modal, Order, RichText, Sense, Stroke, Vec2, pos2,
};
use egui_file_dialog::{DialogState, FileDialog as EguiFileDialog, Filter};
use euclid::Length;
use log::{error, warn};
use servo::{
    AlertDialog, AuthenticationRequest, BluetoothDeviceSelectionRequest, ColorPicker,
    ConfirmDialog, ContextMenu, ContextMenuItem, DatePicker, DateValue, DeviceIndependentPixel,
    EmbedderControlId, FilePicker, PermissionRequest, PromptDialog, RgbColor, SelectElement,
    SelectElementOption, SelectElementOptionOrOptgroup, SimpleDialog,
};
use time::Month;

/// The minimum width of many UI elements including dialog boxes and menus,
/// for the sake of consistency.
const MINIMUM_UI_ELEMENT_WIDTH: f32 = 150.0;

/// The minimum width of the calendar shown by the date picker dialog, which
/// needs to fit seven day columns.
const DATE_PICKER_MINIMUM_WIDTH: f32 = 230.0;

/// English month names, indexed by `time::Month` minus one.
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Weekday column headers for the date picker, starting on Sunday.
const WEEKDAY_LABELS: [&str; 7] = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];

#[expect(clippy::large_enum_variant)]
pub enum Dialog {
    File {
        dialog: EguiFileDialog,
        maybe_picker: Option<FilePicker>,
    },
    Alert(Option<AlertDialog>),
    Confirm(Option<ConfirmDialog>),
    Prompt(Option<PromptDialog>),
    Authentication {
        username: String,
        password: String,
        request: Option<AuthenticationRequest>,
    },
    Permission {
        message: String,
        request: Option<PermissionRequest>,
    },
    SelectDevice {
        request: Option<BluetoothDeviceSelectionRequest>,
        selected_device_index: usize,
    },
    SelectElement {
        maybe_prompt: Option<SelectElement>,
        toolbar_offset: Length<f32, DeviceIndependentPixel>,
    },
    ColorPicker {
        current_color: egui::Color32,
        maybe_prompt: Option<ColorPicker>,
        toolbar_offset: Length<f32, DeviceIndependentPixel>,
    },
    DatePicker {
        maybe_prompt: Option<DatePicker>,
        toolbar_offset: Length<f32, DeviceIndependentPixel>,
        /// The month that is currently displayed by the calendar.
        displayed_month: (i32, Month),
    },
    ContextMenu {
        menu: Option<ContextMenu>,
        toolbar_offset: Length<f32, DeviceIndependentPixel>,
    },
}

impl Dialog {
    pub fn new_file_dialog(file_picker: FilePicker) -> Self {
        let mut dialog = EguiFileDialog::new();
        if !file_picker.filter_patterns().is_empty() {
            let filter_patterns = file_picker.filter_patterns().to_owned();
            let filter = Filter::new(move |path: &Path| {
                path.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|ext| {
                        let ext = ext.to_lowercase();
                        filter_patterns.iter().any(|pattern| ext == pattern.0)
                    })
            });
            dialog = dialog
                .add_file_filter("All Supported Types", filter)
                .default_file_filter("All Supported Types");
        }

        Dialog::File {
            dialog,
            maybe_picker: Some(file_picker),
        }
    }

    pub fn new_simple_dialog(dialog: SimpleDialog) -> Self {
        match dialog {
            SimpleDialog::Alert(alert_dialog) => Self::Alert(Some(alert_dialog)),
            SimpleDialog::Confirm(confirm_dialog) => Self::Confirm(Some(confirm_dialog)),
            SimpleDialog::Prompt(prompt_dialog) => Self::Prompt(Some(prompt_dialog)),
        }
    }

    pub fn new_authentication_dialog(authentication_request: AuthenticationRequest) -> Self {
        Dialog::Authentication {
            username: String::new(),
            password: String::new(),
            request: Some(authentication_request),
        }
    }

    pub fn new_permission_request_dialog(permission_request: PermissionRequest) -> Self {
        let message = format!(
            "Do you want to grant permission for {:?}?",
            permission_request.feature()
        );
        Dialog::Permission {
            message,
            request: Some(permission_request),
        }
    }

    pub fn new_device_selection_dialog(request: BluetoothDeviceSelectionRequest) -> Self {
        Dialog::SelectDevice {
            request: Some(request),
            selected_device_index: 0,
        }
    }

    pub fn new_select_element_dialog(
        prompt: SelectElement,
        toolbar_offset: Length<f32, DeviceIndependentPixel>,
    ) -> Self {
        Dialog::SelectElement {
            maybe_prompt: Some(prompt),
            toolbar_offset,
        }
    }

    pub fn new_color_picker_dialog(
        prompt: ColorPicker,
        toolbar_offset: Length<f32, DeviceIndependentPixel>,
    ) -> Self {
        let current_color = prompt
            .current_color()
            .map(|color| egui::Color32::from_rgb(color.red, color.green, color.blue))
            .unwrap_or_default();
        Dialog::ColorPicker {
            current_color,
            maybe_prompt: Some(prompt),
            toolbar_offset,
        }
    }

    pub fn new_date_picker_dialog(
        prompt: DatePicker,
        toolbar_offset: Length<f32, DeviceIndependentPixel>,
    ) -> Self {
        let today = time::OffsetDateTime::now_utc().date();
        let displayed_month = prompt
            .current_date()
            .map(|date| {
                (
                    date.year,
                    Month::try_from(date.month).unwrap_or(today.month()),
                )
            })
            .unwrap_or((today.year(), today.month()));
        Dialog::DatePicker {
            maybe_prompt: Some(prompt),
            toolbar_offset,
            displayed_month,
        }
    }

    /// Returns false if the dialog has been closed, or true otherwise.
    pub fn update(&mut self, ctx: &egui::Context) -> bool {
        enum DialogAction {
            Dismiss,
            Submit,
            Continue,
        }

        match self {
            Dialog::File {
                dialog,
                maybe_picker,
            } => {
                let action = maybe_picker
                    .as_mut()
                    .map(|picker| {
                        if *dialog.state() == DialogState::Closed {
                            if picker.allow_select_multiple() {
                                dialog.pick_multiple();
                            } else {
                                dialog.pick_file();
                            }
                        }

                        let state = dialog.update(ctx).state();
                        match state {
                            DialogState::Open => DialogAction::Continue,
                            DialogState::Picked(path) => {
                                let paths = std::slice::from_ref(path);
                                picker.select(paths);
                                DialogAction::Submit
                            },
                            DialogState::PickedMultiple(paths) => {
                                picker.select(paths);
                                DialogAction::Submit
                            },
                            DialogState::Cancelled | DialogState::Closed => DialogAction::Dismiss,
                        }
                    })
                    .unwrap_or(DialogAction::Dismiss);

                match action {
                    DialogAction::Dismiss => {
                        if let Some(picker) = maybe_picker.take() {
                            picker.dismiss();
                        }
                    },
                    DialogAction::Submit => {
                        if let Some(picker) = maybe_picker.take() {
                            picker.submit();
                        }
                    },
                    DialogAction::Continue => {},
                }
                matches!(action, DialogAction::Continue)
            },
            Dialog::Alert(maybe_alert_dialog) => {
                let Some(alert_dialog) = maybe_alert_dialog else {
                    return false;
                };

                let mut is_open = true;
                Modal::new("Alert".into()).show(ctx, |ui| {
                    make_dialog_label(alert_dialog.message(), ui, None);
                    egui::Sides::new().show(
                        ui,
                        |_ui| {},
                        |ui| {
                            if ui.button("Close").clicked() ||
                                ui.input(|i| i.key_pressed(egui::Key::Escape))
                            {
                                is_open = false;
                            }
                        },
                    );
                });

                if !is_open && let Some(alert_dialog) = maybe_alert_dialog.take() {
                    alert_dialog.confirm();
                }
                is_open
            },
            Dialog::Confirm(maybe_confirm_dialog) => {
                let Some(confirm_dialog) = maybe_confirm_dialog else {
                    return false;
                };

                let mut dialog_action = DialogAction::Continue;
                Modal::new("Confirm".into()).show(ctx, |ui| {
                    make_dialog_label(confirm_dialog.message(), ui, None);
                    egui::Sides::new().show(
                        ui,
                        |_ui| {},
                        |ui| {
                            if ui.button("Ok").clicked() ||
                                ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                dialog_action = DialogAction::Submit;
                            }
                            if ui.button("Cancel").clicked() ||
                                ui.input(|i| i.key_pressed(egui::Key::Escape))
                            {
                                dialog_action = DialogAction::Dismiss;
                            }
                        },
                    );
                });

                match dialog_action {
                    DialogAction::Dismiss => {
                        if let Some(confirm_dialog) = maybe_confirm_dialog.take() {
                            confirm_dialog.dismiss();
                        }
                        false
                    },
                    DialogAction::Submit => {
                        if let Some(confirm_dialog) = maybe_confirm_dialog.take() {
                            confirm_dialog.confirm();
                        }
                        false
                    },
                    DialogAction::Continue => true,
                }
            },
            Dialog::Prompt(maybe_prompt_dialog) => {
                let Some(prompt_dialog) = maybe_prompt_dialog else {
                    return false;
                };

                let mut dialog_action = DialogAction::Continue;
                Modal::new("Prompt".into()).show(ctx, |ui| {
                    let mut prompt_text = prompt_dialog.current_value().to_owned();
                    make_dialog_label(prompt_dialog.message(), ui, Some(&mut prompt_text));
                    egui::Sides::new().show(
                        ui,
                        |_ui| {},
                        |ui| {
                            if ui.button("Ok").clicked() ||
                                ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                prompt_dialog.set_current_value(&prompt_text);
                                dialog_action = DialogAction::Submit;
                            }
                            if ui.button("Cancel").clicked() ||
                                ui.input(|i| i.key_pressed(egui::Key::Escape))
                            {
                                dialog_action = DialogAction::Dismiss;
                            }
                        },
                    );
                    prompt_dialog.set_current_value(&prompt_text);
                });
                match dialog_action {
                    DialogAction::Dismiss => {
                        if let Some(prompt_dialog) = maybe_prompt_dialog.take() {
                            prompt_dialog.dismiss();
                        }
                        false
                    },
                    DialogAction::Submit => {
                        if let Some(prompt_dialog) = maybe_prompt_dialog.take() {
                            prompt_dialog.confirm();
                        }
                        false
                    },
                    DialogAction::Continue => true,
                }
            },
            Dialog::Authentication {
                username,
                password,
                request,
            } => {
                let mut is_open = true;
                Modal::new("authentication".into()).show(ctx, |ui| {
                    let mut frame = egui::Frame::default().inner_margin(10.0).begin(ui);
                    frame.content_ui.set_min_width(MINIMUM_UI_ELEMENT_WIDTH);

                    if let Some(request) = request {
                        let url =
                            egui::RichText::new(request.url().origin().unicode_serialization());
                        frame.content_ui.heading(url);
                    }

                    frame.content_ui.add_space(10.0);

                    frame
                        .content_ui
                        .label("This site is asking you to sign in.");
                    frame.content_ui.add_space(10.0);

                    frame.content_ui.label("Username:");
                    frame.content_ui.text_edit_singleline(username);
                    frame.content_ui.add_space(10.0);

                    frame.content_ui.label("Password:");
                    frame
                        .content_ui
                        .add(egui::TextEdit::singleline(password).password(true));

                    frame.end(ui);

                    egui::Sides::new().show(
                        ui,
                        |_ui| {},
                        |ui| {
                            if ui.button("Sign in").clicked() ||
                                ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                let request =
                                    request.take().expect("non-None until dialog is closed");
                                request.authenticate(username.clone(), password.clone());
                                is_open = false;
                            }
                            if ui.button("Cancel").clicked() ||
                                ui.input(|i| i.key_pressed(egui::Key::Escape))
                            {
                                is_open = false;
                            }
                        },
                    );
                });
                is_open
            },
            Dialog::Permission { message, request } => {
                let mut is_open = true;
                let modal = Modal::new("permission".into());
                modal.show(ctx, |ui| {
                    make_dialog_label(message, ui, None);
                    egui::Sides::new().show(
                        ui,
                        |_ui| {},
                        |ui| {
                            if ui.button("Allow").clicked() ||
                                ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                let request =
                                    request.take().expect("non-None until dialog is closed");
                                request.allow();
                                is_open = false;
                            }
                            if ui.button("Deny").clicked() ||
                                ui.input(|i| i.key_pressed(egui::Key::Escape))
                            {
                                let request =
                                    request.take().expect("non-None until dialog is closed");
                                request.deny();
                                is_open = false;
                            }
                        },
                    );
                });
                is_open
            },
            Dialog::SelectDevice {
                request,
                selected_device_index,
            } => {
                let mut is_open = true;
                let modal = Modal::new("device_picker".into());
                modal.show(ctx, |ui| {
                    if let Some(request) = request {
                        let mut frame = egui::Frame::default().inner_margin(10.0).begin(ui);
                        frame.content_ui.set_min_width(MINIMUM_UI_ELEMENT_WIDTH);

                        frame.content_ui.heading("Choose a Device");
                        frame.content_ui.add_space(10.0);

                        let devices = request.devices();
                        let combo_box =
                            if let Some(selected_device) = devices.get(*selected_device_index) {
                                egui::ComboBox::from_label("")
                                    .selected_text(selected_device.name.clone())
                            } else {
                                egui::ComboBox::from_label("")
                            };

                        combo_box.show_ui(&mut frame.content_ui, |ui| {
                            for (i, device) in devices.iter().enumerate() {
                                ui.selectable_value(selected_device_index, i, device.name.clone());
                            }
                        });

                        frame.end(ui);
                    } else {
                        error!("Unexpected: None SelectDevice while the dialog is open");
                    }

                    egui::Sides::new().show(
                        ui,
                        |_ui| {},
                        |ui| {
                            let ok_button = egui::Button::new("Ok");
                            let has_selected_device = request.as_ref().is_some_and(|request| {
                                request.devices().get(*selected_device_index).is_some()
                            });
                            if ui.add_enabled(has_selected_device, ok_button).clicked() ||
                                (has_selected_device &&
                                    ui.input(|i| i.key_pressed(egui::Key::Enter)))
                            {
                                let request =
                                    request.take().expect("non-None until dialog is closed");
                                let choice = &request.devices()[*selected_device_index].clone();
                                if let Err(error) = request.pick_device(choice) {
                                    warn!("Failed to send device selection: {error}");
                                }
                                is_open = false;
                            }
                            if ui.button("Cancel").clicked() ||
                                ui.input(|i| i.key_pressed(egui::Key::Escape))
                            {
                                let request =
                                    request.take().expect("non-None until dialog is closed");
                                if let Err(error) = request.cancel() {
                                    warn!("Failed to send cancellation: {error}");
                                }
                                is_open = false;
                            }
                        },
                    );
                });
                is_open
            },
            Dialog::SelectElement {
                maybe_prompt,
                toolbar_offset,
            } => {
                let Some(prompt) = maybe_prompt else {
                    // Prompt was dismissed, so the dialog should be closed too.
                    return false;
                };
                let mut is_open = true;

                let mut position = prompt.position();
                position.min.y += toolbar_offset.0 as i32;
                position.max.y += toolbar_offset.0 as i32;
                let area = egui::Area::new(egui::Id::new("select-window"))
                    .fixed_pos(egui::pos2(position.min.x as f32, position.max.y as f32));

                let mut selected_options = prompt.selected_options();

                fn display_option(
                    ui: &mut egui::Ui,
                    option: &SelectElementOption,
                    selected_options: &mut Vec<usize>,
                    is_open: &mut bool,
                    in_group: bool,
                    allow_multiple: bool,
                ) {
                    let is_checked = selected_options.contains(&option.id);

                    // TODO: Surely there's a better way to align text in a selectable label in egui.
                    let label_text = if in_group {
                        format!("   {}", option.label)
                    } else {
                        option.label.to_owned()
                    };
                    let label = if option.is_disabled {
                        egui::RichText::new(&label_text).strikethrough()
                    } else {
                        egui::RichText::new(&label_text)
                    };
                    let clickable_area = ui
                        .allocate_ui_with_layout(
                            [ui.available_width(), 0.0].into(),
                            egui::Layout::top_down_justified(egui::Align::LEFT),
                            |ui| ui.selectable_label(is_checked, label),
                        )
                        .inner;

                    if clickable_area.clicked() && !option.is_disabled {
                        if allow_multiple {
                            if let Some(position) =
                                selected_options.iter().position(|id| *id == option.id)
                            {
                                selected_options.remove(position);
                            } else {
                                selected_options.push(option.id);
                            }
                        } else {
                            selected_options.clear();
                            selected_options.push(option.id);
                        }
                        if !allow_multiple {
                            *is_open = false;
                        }
                    }

                    if clickable_area.hovered() && option.is_disabled {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
                    }

                    if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
                        *is_open = false;
                    }
                }

                let modal = Modal::new("select_element_picker".into()).area(area);
                let backdrop_response = modal
                    .show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for option_or_optgroup in prompt.options() {
                                match &option_or_optgroup {
                                    SelectElementOptionOrOptgroup::Option(option) => {
                                        display_option(
                                            ui,
                                            option,
                                            &mut selected_options,
                                            &mut is_open,
                                            false,
                                            prompt.allow_select_multiple(),
                                        );
                                    },
                                    SelectElementOptionOrOptgroup::Optgroup { label, options } => {
                                        ui.label(RichText::new(label).strong());

                                        for option in options {
                                            display_option(
                                                ui,
                                                option,
                                                &mut selected_options,
                                                &mut is_open,
                                                true,
                                                prompt.allow_select_multiple(),
                                            );
                                        }
                                    },
                                }
                            }
                        });
                    })
                    .backdrop_response;

                // FIXME: Doesn't update until you move your mouse or press a key - why?
                if backdrop_response.clicked() {
                    is_open = false;
                }

                prompt.select(selected_options);

                if !is_open {
                    maybe_prompt.take().unwrap().submit();
                }

                is_open
            },
            Dialog::ColorPicker {
                current_color,
                maybe_prompt,
                toolbar_offset,
            } => {
                let Some(prompt) = maybe_prompt else {
                    // Prompt was dismissed, so the dialog should be closed too.
                    return false;
                };
                let mut is_open = true;

                let mut position = prompt.position();
                position.min.y += toolbar_offset.0 as i32;
                position.max.y += toolbar_offset.0 as i32;
                let area = egui::Area::new(egui::Id::new("select-window"))
                    .fixed_pos(egui::pos2(position.min.x as f32, position.max.y as f32));

                let modal = Modal::new("select_element_picker".into()).area(area);
                let backdrop_response = modal
                    .show(ctx, |ui| {
                        egui::widgets::color_picker::color_picker_color32(
                            ui,
                            current_color,
                            egui::widgets::color_picker::Alpha::Opaque,
                        );

                        ui.add_space(10.);

                        if ui.button("Dismiss").clicked() ||
                            ui.input(|i| i.key_pressed(egui::Key::Escape))
                        {
                            is_open = false;
                            prompt.select(None);
                        }
                        if ui.button("Select").clicked() {
                            is_open = false;
                            let selected_color = RgbColor {
                                red: current_color.r(),
                                green: current_color.g(),
                                blue: current_color.b(),
                            };
                            prompt.select(Some(selected_color));
                        }
                    })
                    .backdrop_response;

                // FIXME: Doesn't update until you move your mouse or press a key - why?
                if backdrop_response.clicked() {
                    is_open = false;
                }

                is_open
            },
            Dialog::DatePicker {
                maybe_prompt,
                toolbar_offset,
                displayed_month,
            } => {
                let Some(prompt) = maybe_prompt else {
                    // Prompt was dismissed, so the dialog should be closed too.
                    return false;
                };
                let mut is_open = true;
                // The action to resolve the prompt with, if any: `None` means the picker is
                // dismissed without a change, `Some(None)` clears the value, and
                // `Some(Some(date))` selects a date.
                let mut action: Option<Option<DateValue>> = None;

                let current_date = prompt.current_date();
                let mut position = prompt.position();
                position.min.y += toolbar_offset.0 as i32;
                position.max.y += toolbar_offset.0 as i32;
                let area = egui::Area::new(egui::Id::new("date-picker"))
                    .fixed_pos(egui::pos2(position.min.x as f32, position.max.y as f32));

                let modal = egui::Modal::new("date_picker".into()).area(area);
                let backdrop_response = modal
                    .show(ctx, |ui| {
                        ui.set_min_width(DATE_PICKER_MINIMUM_WIDTH);

                        let (year, month) = *displayed_month;

                        // Header with the displayed month and buttons to navigate to the
                        // previous and next month. The label must not be centered with
                        // `ui.with_layout` here: a layout that fills the available width
                        // makes the auto-sized popup area grow on every frame, since the
                        // area size of a frame is derived from the minimum size of the
                        // previous one.
                        ui.horizontal(|ui| {
                            if ui.button("‹").clicked() {
                                *displayed_month = previous_month(year, month);
                            }
                            ui.label(
                                RichText::new(format!(
                                    "{} {}",
                                    MONTH_NAMES[(u8::from(month) - 1) as usize],
                                    year
                                ))
                                .strong(),
                            );
                            if ui.button("›").clicked() {
                                *displayed_month = next_month(year, month);
                            }
                        });

                        ui.add_space(4.0);

                        // The day grid: a row of weekday headers followed by one row per
                        // week, with empty cells before the first day of the month.
                        let days_in_month = time::util::days_in_month(month, year);
                        let first_weekday = time::Date::from_calendar_date(year, month, 1)
                            .map(|date| date.weekday().number_days_from_sunday())
                            .unwrap_or(0);
                        let today = time::OffsetDateTime::now_utc().date();

                        egui::Grid::new("date-picker-days").show(ui, |ui| {
                            for weekday_label in WEEKDAY_LABELS {
                                ui.label(RichText::new(weekday_label).weak());
                            }
                            ui.end_row();

                            for _ in 0..first_weekday {
                                ui.label("");
                            }

                            for day in 1..=days_in_month {
                                let is_today = (year, u8::from(month), day)
                                    == (today.year(), u8::from(today.month()), today.day());
                                let is_selected = current_date.is_some_and(|date| {
                                    (date.year, date.month, date.day)
                                        == (year, u8::from(month), day)
                                });

                                let mut button = egui::Button::new(RichText::new(day.to_string()))
                                    .min_size(Vec2::new(28.0, 20.0));
                                if is_selected {
                                    button = button.fill(ui.visuals().selection.bg_fill);
                                }
                                if is_today {
                                    button = button
                                        .stroke(Stroke::new(1.0, ui.visuals().strong_text_color()));
                                }

                                if ui.add(button).clicked() {
                                    action = Some(Some(DateValue {
                                        year,
                                        month: u8::from(month),
                                        day,
                                    }));
                                    is_open = false;
                                }

                                if (first_weekday as u32 + day as u32 - 1) % 7 == 6 {
                                    ui.end_row();
                                }
                            }
                        });

                        ui.add_space(4.0);

                        ui.horizontal(|ui| {
                            if ui.button("Today").clicked() {
                                action = Some(Some(DateValue {
                                    year: today.year(),
                                    month: u8::from(today.month()),
                                    day: today.day(),
                                }));
                                is_open = false;
                            }
                            if ui.button("Clear").clicked() {
                                action = Some(None);
                                is_open = false;
                            }
                            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                is_open = false;
                            }
                        });
                    })
                    .backdrop_response;

                // FIXME: Doesn't update until you move your mouse or press a key - why?
                if backdrop_response.clicked() {
                    is_open = false;
                }

                if !is_open {
                    let mut prompt = maybe_prompt.take().unwrap();
                    if let Some(selection) = action {
                        prompt.select(selection);
                        prompt.submit();
                    }
                    // Without an action the prompt is simply dropped, which dismisses it
                    // and leaves the value of the element unchanged.
                }

                is_open
            },
            Dialog::ContextMenu {
                menu,
                toolbar_offset,
            } => {
                let mut is_open = true;
                if let Some(context_menu) = menu {
                    let mut selected_action = None;
                    let mut position = context_menu.position();
                    position.min.y += toolbar_offset.0 as i32;
                    position.max.y += toolbar_offset.0 as i32;

                    let response = Area::new(Id::new("context_menu"))
                        .fixed_pos(pos2(position.min.x as f32, position.min.y as f32))
                        .order(Order::Foreground)
                        .show(ctx, |ui| {
                            Frame::popup(ui.style()).show(ui, |ui| {
                                ui.set_min_width(MINIMUM_UI_ELEMENT_WIDTH);
                                for item in context_menu.items() {
                                    match item {
                                        ContextMenuItem::Item {
                                            label,
                                            action,
                                            enabled,
                                        } => {
                                            let (color, sense) = match enabled {
                                                true => (
                                                    ui.visuals().strong_text_color(),
                                                    Sense::click(),
                                                ),
                                                false => {
                                                    (ui.visuals().weak_text_color(), Sense::empty())
                                                },
                                            };

                                            {
                                                let panel_fill = ui.visuals().panel_fill;
                                                let style = ui.style_mut();
                                                let widgets = &mut style.visuals.widgets;

                                                widgets.inactive.weak_bg_fill = panel_fill;
                                                widgets.inactive.bg_fill = panel_fill;

                                                // Disable stroke for all button types in the menu.
                                                widgets.active.bg_stroke = Stroke::NONE;
                                                widgets.inactive.bg_stroke = Stroke::NONE;
                                                widgets.hovered.bg_stroke = Stroke::NONE;
                                                widgets.noninteractive.bg_stroke = Stroke::NONE;
                                            }

                                            let button =
                                                Button::new(RichText::new(label).color(color))
                                                    .sense(sense)
                                                    .corner_radius(CornerRadius::ZERO)
                                                    .wrap_mode(egui::TextWrapMode::Extend)
                                                    .min_size(Vec2 {
                                                        x: MINIMUM_UI_ELEMENT_WIDTH,
                                                        y: 0.0,
                                                    });

                                            if ui.add(button).clicked() {
                                                selected_action = Some(*action);
                                                ui.close();
                                            }
                                        },
                                        ContextMenuItem::Separator => {
                                            ui.separator();
                                        },
                                    }
                                }
                            })
                        });

                    if response.response.clicked_elsewhere() {
                        is_open = false;
                    }

                    if let Some(action) = selected_action &&
                        let Some(context_menu) = menu.take()
                    {
                        context_menu.select(action);
                        return false;
                    }
                }
                is_open
            },
        }
    }

    pub(crate) fn embedder_control_id(&self) -> Option<EmbedderControlId> {
        match self {
            Dialog::SelectElement { maybe_prompt, .. } => {
                maybe_prompt.as_ref().map(|element| element.id())
            },
            Dialog::ColorPicker { maybe_prompt, .. } => {
                maybe_prompt.as_ref().map(|element| element.id())
            },
            Dialog::DatePicker { maybe_prompt, .. } => {
                maybe_prompt.as_ref().map(|element| element.id())
            },
            Dialog::ContextMenu { menu, .. } => menu.as_ref().map(|menu| menu.id()),
            _ => None,
        }
    }

    pub(crate) fn new_context_menu(
        menu: ContextMenu,
        toolbar_offset: Length<f32, DeviceIndependentPixel>,
    ) -> Dialog {
        Dialog::ContextMenu {
            menu: Some(menu),
            toolbar_offset,
        }
    }
}

fn make_dialog_label(message: &str, ui: &mut egui::Ui, input_text: Option<&mut String>) {
    let mut frame = egui::Frame::default().inner_margin(10.0).begin(ui);
    frame.content_ui.set_min_width(MINIMUM_UI_ELEMENT_WIDTH);
    frame.content_ui.label(message);
    if let Some(input_text) = input_text {
        frame.content_ui.text_edit_singleline(input_text);
    }
    frame.end(ui);
}

fn previous_month(year: i32, month: Month) -> (i32, Month) {
    match month {
        Month::January => (year - 1, Month::December),
        month => (year, month.previous()),
    }
}

fn next_month(year: i32, month: Month) -> (i32, Month) {
    match month {
        Month::December => (year + 1, Month::January),
        month => (year, month.next()),
    }
}

#[cfg(test)]
mod tests {
    use time::Month;

    #[test]
    fn month_navigation_rolls_over_at_year_boundaries() {
        assert_eq!(
            super::previous_month(2026, Month::January),
            (2025, Month::December)
        );
        assert_eq!(
            super::next_month(2026, Month::December),
            (2027, Month::January)
        );
        assert_eq!(
            super::next_month(2026, Month::February),
            (2026, Month::March)
        );
    }
}
