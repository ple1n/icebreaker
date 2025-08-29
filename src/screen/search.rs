use std::collections::HashMap;
use std::sync::Arc;

use crate::core::model;
use crate::core::{Error, HFModel};
use crate::model::Model;
use crate::widget::sidebar;
use crate::{icon, APIAccess};

use icebreaker_core::model::{EndpointId, Library, ModelOnline};
use iced::border;
use iced::font;
use iced::time::Duration;
use iced::widget::{
    self, button, center, center_x, column, container, grid, horizontal_rule, horizontal_space,
    right, row, rule, scrollable, text, text_input, value,
};
use iced::{Center, Element, Fill, Font, Right, Shrink, Task, Theme};
use iced_palace::widget::ellipsized_text;

use function::Binary;

pub struct Search {
    models: HashMap<model::EndpointId, Model>,
    search: String,
    search_temperature: usize,
    is_searching: bool,
    mode: Mode,
    show_filters: bool,
    show_local_models: bool,
    show_online_models: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    ModelsListed(Result<Vec<Model>, Error>),
    SearchChanged(String),
    SearchCooled,
    Select(model::EndpointId),
    HFDetailsFetched(model::EndpointId, Result<model::Details, Error>),
    FilesListed(model::EndpointId, Result<model::Files, Error>),
    Boot(model::FileAndAPI),
    Back,
    ToggleFilters,
    ToggleLocalModels(bool),
    ToggleOnlineModels(bool),
    InstallAPI(model::EndpointId), // Add new message for installing API models
}

pub enum Mode {
    Search,
    HFDetails {
        model: model::EndpointId,
        details: Option<model::Details>,
        files: Option<model::Files>,
    },
    APIDetails {
        model: model::EndpointId,
        model_online: ModelOnline,
    },
}

pub enum Action {
    None,
    Boot(model::FileAndAPI),
    Run(Task<Message>),
}

impl Search {
    pub fn new(lib: Library) -> (Self, Task<Message>) {
        let k = Self {
            models: HashMap::new(),
            search: String::new(),
            search_temperature: 0,
            is_searching: true,
            mode: Mode::Search,
            show_filters: false,
            show_local_models: false,
            show_online_models: true,
        };
        (
            k,
            Task::batch([
                Task::perform(Model::list(lib), Message::ModelsListed),
                widget::focus_next(),
            ]),
        )
    }

    pub fn title(&self) -> &str {
        match &self.mode {
            Mode::Search => "Models",
            Mode::HFDetails { model, .. } => model.slash_id().name(),
            Mode::APIDetails {
                model,
                model_online,
            } => model.slash_id().name(),
        }
    }

    pub fn update(&mut self, message: Message) -> Action {
        match message {
            Message::ModelsListed(Ok(models)) => {
                self.models = models
                    .into_iter()
                    .map(|model| (model.endpoint_id(), model))
                    .collect();
                self.is_searching = false;

                Action::None
            }
            Message::ModelsListed(Err(error)) => {
                log::error!("{error}");

                Action::None
            }
            Message::SearchChanged(search) => {
                self.search = search;
                self.search_temperature += 1;

                Action::Run(Task::perform(
                    tokio::time::sleep(Duration::from_secs(1)),
                    |_| Message::SearchCooled,
                ))
            }
            Message::SearchCooled => {
                self.search_temperature = self.search_temperature.saturating_sub(1);

                if self.search_temperature == 0 {
                    self.is_searching = true;

                    Action::Run(Task::perform(
                        Model::search(self.search.clone()),
                        Message::ModelsListed,
                    ))
                } else {
                    Action::None
                }
            }
            Message::Select(id) => {
                let model = self.models.get(&id);
                if let Some(model) = model {
                    match model {
                        Model::HF(_) => {
                            self.mode = Mode::HFDetails {
                                model: id.clone(),
                                details: None,
                                files: None,
                            };
                            Action::Run(Task::batch([
                                Task::perform(
                                    model::Details::fetch(id.clone()),
                                    Message::HFDetailsFetched.with(id.clone()),
                                ),
                                Task::perform(
                                    model::File::list(id.slash_id().clone()),
                                    Message::FilesListed.with(id.clone()),
                                ),
                            ]))
                        }
                        Model::API(model_online) => {
                            self.mode = Mode::APIDetails {
                                model: id.clone(),
                                model_online: model_online.clone(),
                            };
                            Action::None
                        }
                    }
                } else {
                    log::warn!("select {:?}", &id);
                    Action::None
                }
            }
            Message::HFDetailsFetched(new_model, Ok(new_details)) => {
                match &mut self.mode {
                    Mode::HFDetails { model, details, .. } if model == &new_model => {
                        *details = Some(new_details);
                    }
                    _ => {}
                }

                Action::None
            }
            Message::FilesListed(new_model, Ok(new_files)) => {
                match &mut self.mode {
                    Mode::HFDetails { model, files, .. } if model == &new_model => {
                        *files = Some(new_files);
                    }
                    _ => {}
                }

                Action::None
            }
            Message::Back => {
                self.mode = Mode::Search;

                Action::Run(widget::focus_next())
            }
            Message::Boot(file) => Action::Boot(file),
            Message::HFDetailsFetched(_, Err(error)) | Message::FilesListed(_, Err(error)) => {
                log::error!("{error}");

                Action::None
            }
            Message::ToggleFilters => {
                self.show_filters = !self.show_filters;
                Action::None
            }
            Message::ToggleLocalModels(t) => {
                self.show_local_models = t;
                Action::None
            }
            Message::ToggleOnlineModels(t) => {
                self.show_online_models = t;
                Action::None
            }
            Message::InstallAPI(id) => {
                // Add model to local registry of favorited models
                log::info!("Installing API model: {:?}", id);
                let ap = self.models.get(&id).unwrap();
                let ap = match ap {
                    Model::API(ap) => ap,
                    _ => unreachable!(),
                };
                Action::Boot(model::FileAndAPI {
                    api: Some(ap.clone()),
                    ..Default::default()
                })
            }
        }
    }

    pub fn view<'a>(&'a self, library: &'a model::Library) -> Element<'a, Message> {
        match &self.mode {
            Mode::Search => self.search(),
            Mode::HFDetails {
                model,
                details,
                files,
            } => self.details(model.slash_id(), details.as_ref(), files.as_ref(), library),
            Mode::APIDetails {
                model,
                model_online,
            } => self.details_api(model_online),
        }
    }

    pub fn search(&self) -> Element<'_, Message> {
        let search_row = row![
            text_input("Search language models...", &self.search)
                .size(20)
                .padding(10)
                .on_input(Message::SearchChanged)
                .style(|theme, status| {
                    let style = text_input::default(theme, status);
                    text_input::Style {
                        border: style.border.rounded(5),
                        ..style
                    }
                })
                .width(Fill),
            button(
                container(center(icon::filter().size(16))) // Adjusted icon size
                    .padding(5) // Reduced padding
                    .width(42)
                    .height(42)
            )
            .padding(0)
            .style(|theme, status| {
                let palette = theme.extended_palette();

                button::Style {
                    background: Some(
                        match status {
                            button::Status::Hovered => palette.background.weak.color,
                            button::Status::Pressed => palette.background.strong.color,
                            _ => palette.background.weakest.color,
                        }
                        .into(),
                    ),
                    border: border::rounded(5).width(1).color(match status {
                        button::Status::Hovered => palette.background.strong.color,
                        button::Status::Pressed => palette.background.strongest.color,
                        _ => palette.background.weak.color,
                    }),
                    text_color: palette.background.weak.text,
                    ..button::secondary(theme, status)
                }
            })
            .on_press(Message::ToggleFilters)
        ]
        .spacing(5) // Reduced spacing
        .height(42)
        .align_y(iced::Alignment::Center);

        let filter_panel = self.show_filters.then(|| {
            let local_toggle = widget::toggler(self.show_local_models)
                .label("Local Models".to_string())
                .on_toggle(Message::ToggleLocalModels);

            let online_toggle = widget::toggler(self.show_online_models)
                .label("Online Models".to_string())
                .on_toggle(Message::ToggleOnlineModels);

            container(column![local_toggle, online_toggle].spacing(10))
                .padding(10)
                .style(container::bordered_box)
        });

        let models: Element<'_, _> = {
            let search_terms: Vec<_> = self
                .search
                .trim()
                .split(' ')
                .map(str::to_lowercase)
                .collect();

            let mut filtered_models = self
                .models
                .values()
                .filter(|model| {
                    self.search.is_empty()
                        || search_terms.iter().all(|term| {
                            model.slash_id().name().to_lowercase().contains(term)
                                || model.slash_id().author().to_lowercase().contains(term)
                        })
                })
                .peekable();

            if filtered_models.peek().is_none() {
                center(text(if self.is_searching || self.search_temperature > 0 {
                    "Searching..."
                } else {
                    "No models found!"
                }))
                .into()
            } else {
                let cards = grid(filtered_models.map(model_card))
                    .spacing(10)
                    .fluid(650)
                    .height(Shrink);

                scrollable(cards).height(Fill).spacing(10).into()
            }
        };

        column![search_row, filter_panel, models].spacing(10).into()
    }

    pub fn details<'a>(
        &self,
        model: &'a model::Id,
        details: Option<&'a model::Details>,
        files: Option<&'a model::Files>,
        library: &'a model::Library,
    ) -> Element<'a, Message> {
        use iced::widget::Text;

        let back = button(row![icon::left(), "All models"].align_y(Center).spacing(10))
            .padding([10, 0])
            .on_press(Message::Back)
            .style(button::text);

        fn badge<'a>(icon: Text<'a>, value: Text<'a>) -> Element<'a, Message> {
            container(
                row![
                    icon.size(10).style(text::secondary).line_height(1.0),
                    value.size(12).font(Font::MONOSPACE)
                ]
                .align_y(Center)
                .spacing(5),
            )
            .padding([4, 7])
            .style(container::bordered_box)
            .into()
        }

        let header = {
            let title = center_x(
                row![
                    text(model.author()).size(18),
                    text("/").size(18),
                    ellipsized_text(model.name())
                        .size(20)
                        .font(Font {
                            weight: font::Weight::Semibold,
                            ..Font::MONOSPACE
                        })
                        .wrapping(text::Wrapping::None)
                ]
                .align_y(Center)
                .spacing(5),
            );

            let badges = details.map(|details| {
                row![
                    badge(icon::sliders(), value(details.parameters)),
                    details
                        .architecture
                        .as_ref()
                        .map(|architecture| badge(icon::server(), text(architecture))),
                    badge(icon::star(), value(details.likes)),
                    badge(icon::download(), value(details.downloads)),
                    badge(
                        icon::clock(),
                        value(details.last_modified.format("%-e %B, %Y")),
                    ),
                ]
                .align_y(Center)
                .spacing(10)
            });

            column![title, badges].spacing(10).align_x(Center)
        };

        let download = files.map(|files| view_files(files, library));

        scrollable(center_x(
            column![back, header, download]
                .spacing(20)
                .max_width(600)
                .clip(true),
        ))
        .spacing(10)
        .into()
    }

    pub fn details_api<'a>(
        &self,
        model_online: &'a ModelOnline,
    ) -> Element<'a, Message> {
        use iced::widget::Text;

        let back = button(row![icon::left(), "All models"].align_y(Center).spacing(10))
            .padding([10, 0])
            .on_press(Message::Back)
            .style(button::text);

        fn badge<'a>(icon: Text<'a>, value: Text<'a>) -> Element<'a, Message> {
            container(
                row![
                    icon.size(10).style(text::secondary).line_height(1.0),
                    value.size(12).font(Font::MONOSPACE)
                ]
                .align_y(Center)
                .spacing(5),
            )
            .padding([4, 7])
            .style(container::bordered_box)
            .into()
        }

        let header = {
            let title = center_x(
                row![
                    text(model_online.endpoint_id.slash_id().author()).size(18),
                    text("/").size(18),
                    ellipsized_text(model_online.endpoint_id.slash_id().name())
                        .size(20)
                        .font(Font {
                            weight: font::Weight::Semibold,
                            ..Font::MONOSPACE
                        })
                        .wrapping(text::Wrapping::None)
                ]
                .align_y(Center)
                .spacing(5),
            );

            let badges = row![
                badge(icon::cloud(), text(format!("{:?}", model_online.config.kind))),
                model_online.cost.as_ref().map(|cost| {
                    row![
                        badge(icon::dollar(), value(cost.prompt.clone())),
                        badge(icon::dollar(), value(cost.completion.clone())),
                    ]
                    .spacing(10)
                }),
            ]
            .align_y(Center)
            .spacing(10);

            column![title, badges].spacing(10).align_x(Center)
        };

        let install_button = button("Install")
            .padding([10, 20])
            .on_press(Message::InstallAPI(model_online.endpoint_id.clone()))
            .style(|theme: &Theme, status| {
                let palette = theme.extended_palette();
                let base = button::primary(theme, status);
                button::Style {
                    background: base.background.map(|bg| {
                        match status {
                            button::Status::Hovered => palette.primary.weak.color,
                            button::Status::Pressed => palette.primary.strong.color,
                            _ => palette.primary.base.color,
                        }
                        .into()
                    }),
                    ..base
                }
            });

        scrollable(center_x(
            column![back, header, install_button]
                .spacing(20)
                .max_width(600)
                .clip(true),
        ))
        .spacing(10)
        .into()
    }

    pub fn sidebar<'a>(&'a self, library: &'a model::Library) -> Element<'a, Message> {
        let header = sidebar::header("Models", Some((icon::search(), Message::Back)));

        if library.files.is_empty() {
            return column![
                header,
                center(
                    text("No models have been downloaded yet.\n\nFind some to start chatting →")
                        .width(Fill)
                        .center()
                        .shaping(text::Shaping::Advanced)
                )
            ]
            .spacing(10)
            .into();
        }

        let library = column(library.files.iter().map(|(fid, file)| {
            use model::*;

            let title: Element<'_, _> = match file {
                FileOrAPI::API(a) => widget::text!("{:?}", &a.endpoint_id.slash_id()).into(),
                FileOrAPI::File(f) => ellipsized_text(f.model.name())
                    .font(Font::MONOSPACE)
                    .wrapping(text::Wrapping::None)
                    .into(),
            };

            let author = match file {
                FileOrAPI::API(a) => row![
                    icon::cloud()
                        .size(10)
                        .line_height(1.0)
                        .style(text::secondary),
                    text(format!("{:?}", &a.config.kind))
                        .size(12)
                        .style(text::secondary),
                ]
                .spacing(5)
                .align_y(Center),
                FileOrAPI::File(f) => row![
                    icon::user()
                        .size(10)
                        .line_height(1.0)
                        .style(text::secondary),
                    text(f.model.author()).size(12).style(text::secondary),
                ]
                .spacing(5)
                .align_y(Center),
            };

            let variant = match file {
                FileOrAPI::API(a) => None,
                FileOrAPI::File(file) => Some(file.variant().map(|variant| {
                    text(variant)
                        .font(Font::MONOSPACE)
                        .size(12)
                        .style(text::secondary)
                })),
            };

            let entry = column![
                title,
                row![author, horizontal_space(), variant]
                    .spacing(5)
                    .align_y(Center)
            ]
            .spacing(2);

            let is_active = match &self.mode {
                Mode::HFDetails { model, .. } => match file {
                    FileOrAPI::File(f) => model == &f.endpoint(),
                    _ => false,
                },
                Mode::APIDetails {
                    model,
                    model_online,
                } => match file {
                    FileOrAPI::API(a) => a == model_online,
                    _ => false,
                },
                _ => false,
            };

            sidebar::item(entry, is_active, || {
                Message::Select(fid.clone())
            })
        }));

        column![header, scrollable(library).spacing(10).height(Fill)]
            .spacing(10)
            .into()
    }
}

fn model_card(model: &Model) -> Element<'_, Message> {
    use iced::widget::Text;

    fn stat<'a>(
        icon: Text<'a>,
        value: Text<'a>,
        style: fn(&Theme) -> text::Style,
    ) -> Element<'a, Message> {
        row![
            icon.size(10).line_height(1.0).style(style),
            value.size(12).font(Font::MONOSPACE).style(style)
        ]
        .align_y(Center)
        .spacing(5)
        .into()
    }
    match model {
        Model::HF(model) => {
            let title = ellipsized_text(model.id.name())
                .font(Font::MONOSPACE)
                .wrapping(text::Wrapping::None);

            let metadata = row![
                stat(icon::user(), text(model.id.author()), text::secondary),
                // Hide when None: format inside the map to avoid calling on Option
                stat(
                    icon::clock(),
                    value(model.last_modified.format("%-e %B, %y")),
                    text::secondary,
                ),
                stat(icon::download(), value(model.downloads), text::primary),
                stat(icon::star(), value(model.likes), text::warning),
            ]
            .spacing(20);

            button(column![title, metadata].spacing(10))
                .width(Fill)
                .padding(10)
                .style(|theme, status| {
                    let palette = theme.extended_palette();

                    let base = button::Style {
                        background: Some(palette.background.weakest.color.into()),
                        text_color: palette.background.weakest.text,
                        border: border::rounded(5)
                            .color(palette.background.weak.color)
                            .width(1),
                        ..button::Style::default()
                    };

                    match status {
                        button::Status::Active | button::Status::Disabled => base,
                        button::Status::Hovered => button::Style {
                            background: Some(palette.background.weak.color.into()),
                            text_color: palette.background.weak.text,
                            border: base.border.color(palette.background.strong.color),
                            ..base
                        },
                        button::Status::Pressed => button::Style {
                            border: base.border.color(palette.background.strongest.color),
                            ..base
                        },
                    }
                })
                .on_press_with(|| Message::Select(model.endpoint_id()))
                .into()
        }
        Model::API(model) => {
            let title = ellipsized_text(model.endpoint_id.slash_id().name())
                .font(Font::MONOSPACE)
                .wrapping(text::Wrapping::None);

            let metadata = row![
                stat(icon::user(), text(model.endpoint_id.slash_id().author()), text::secondary),
                stat(
                    icon::cloud(),
                    text(format!("{:?}", model.config.kind)),
                    text::secondary,
                ),
                model.cost.as_ref().map(|cost| {
                    row![
                        stat(icon::dollar(), value(cost.prompt.clone()), text::secondary),
                        stat(
                            icon::dollar(),
                            value(cost.completion.clone()),
                            text::secondary
                        ),
                    ]
                    .spacing(10)
                }),
            ]
            .spacing(20);

            button(column![title, metadata].spacing(10))
                .width(Fill)
                .padding(10)
                .style(|theme, status| {
                    let palette = theme.extended_palette();

                    let base = button::Style {
                        background: Some(palette.background.weakest.color.into()),
                        text_color: palette.background.weakest.text,
                        border: border::rounded(5)
                            .color(palette.background.weak.color)
                            .width(1),
                        ..button::Style::default()
                    };

                    match status {
                        button::Status::Active | button::Status::Disabled => base,
                        button::Status::Hovered => button::Style {
                            background: Some(palette.background.weak.color.into()),
                            text_color: palette.background.weak.text,
                            border: base.border.color(palette.background.strong.color),
                            ..base
                        },
                        button::Status::Pressed => button::Style {
                            border: base.border.color(palette.background.strongest.color),
                            ..base
                        },
                    }
                })
                .on_press_with(|| Message::Select(model.endpoint_id.clone()))
                .into()
        }
    }
}

pub fn view_files<'a>(
    files: &'a model::Files,
    library: &'a model::Library,
) -> Element<'a, Message> {
    use itertools::Itertools;

    fn view_file<'a>(
        file: &'a model::File,
        library: &'a model::Library,
    ) -> Option<Element<'a, Message>> {
        let variant = file.variant()?;
        let is_ready = library.files.contains_key(&file.endpoint());

        Some(
            button(
                row![
                    is_ready.then(|| icon::check().style(text::primary).size(12)),
                    text(variant)
                        .font(Font::MONOSPACE)
                        .size(12)
                        .style(if is_ready {
                            text::primary
                        } else {
                            text::default
                        }),
                    file.size.map(|size| value(size)
                        .font(Font::MONOSPACE)
                        .size(10)
                        .style(text::secondary))
                ]
                .align_y(Center)
                .spacing(5),
            )
            .on_press_with(|| Message::Boot(model::FileAndAPI {
                file: Some(file.clone()), ..Default::default()
            }))
            .style(move |theme, status| {
                let base = button::background(theme, status);

                if is_ready {
                    button::Style {
                        border: base.border.color(theme.palette().primary).width(1),
                        ..base
                    }
                } else {
                    base
                }
            })
            .into(),
        )
    }

    let files: Element<'_, _> = if files.is_empty() {
        container(
            text("No compatible files have been found for this model.")
                .width(Fill)
                .center(),
        )
        .padding(20)
        .into()
    } else {
        let files = files.iter().map(|(bit, variants)| {
            row![
                value(bit).font(Font::MONOSPACE).size(14).width(80),
                right(
                    row(variants.iter().filter_map(|file| view_file(file, library)))
                        .spacing(10)
                        .wrap()
                        .align_x(Right)
                ),
            ]
            .align_y(Center)
            .into()
        });

        column(Itertools::intersperse_with(files, || {
            horizontal_rule(1).style(rule::weak).into()
        }))
        .spacing(10)
        .into()
    };

    container(files)
        .padding(10)
        .style(container::bordered_box)
        .into()
}
