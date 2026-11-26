//! Workshop browsing and delivery through the game's initialized Steam client.
//!
//! Reading results and delivering content are independent: looking at another
//! page never waits for, or cancels, a download.

use super::model::{ItemView, plural, size};
use crate::{
    lua::{Context, Result},
    steam,
};
use serde_json::json;
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

const QUERY_TIMEOUT: Duration = Duration::from_secs(45);
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(90);
const TYPING_PAUSE: Duration = Duration::from_millis(400);
const PAGE: usize = 50;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Source {
    #[default]
    Subscribed,
    Everything,
}

enum Reading {
    Subscribed {
        query: steam::Query,
        next: usize,
    },
    Search {
        query: steam::Query,
        page: u32,
        text: String,
    },
}

enum Delivery {
    Subscribe(steam::Subscription),
    Download(steam::Download),
}

#[derive(Default)]
pub struct Workshop {
    steam: Option<steam::SteamWorkshop>,
    pub source: Source,
    subscribed: Vec<steam::ItemId>,
    subscribed_items: Vec<steam::WorkshopItem>,
    found: Vec<steam::WorkshopItem>,
    unavailable: Vec<(Source, steam::UnavailableItem)>,
    total: u32,
    searched: Option<String>,
    page: u32,
    reading: Option<Reading>,
    typed: Option<(String, Instant)>,
    delivery: Option<Delivery>,
    pub problem: Option<String>,
}

pub enum Progress {
    /// The item list changed.
    Items,
    Status(String),
    Delivered(u64),
    Failed(String),
}

impl Workshop {
    pub fn attach(context: &mut Context<'_, '_>) -> Self {
        let steam = super::kit::working_directory(context).and_then(|directory| {
            steam::SteamWorkshop::attach_existing(
                context.env,
                &directory.join("natives/libsteam_api.so"),
            )
            .map_err(|error| error.to_string())
        });
        if let Err(reason) = &steam {
            crate::log::info("workshop.unavailable", reason.clone()).write();
        }
        Self {
            steam: steam.ok(),
            source: Source::Subscribed,
            subscribed: Vec::new(),
            subscribed_items: Vec::new(),
            found: Vec::new(),
            unavailable: Vec::new(),
            total: 0,
            searched: None,
            page: 0,
            reading: None,
            typed: None,
            delivery: None,
            problem: None,
        }
    }

    pub fn available(&self) -> bool {
        self.steam.is_some()
    }

    fn steam(&self) -> Result<&steam::SteamWorkshop> {
        self.steam
            .as_ref()
            .ok_or("Steam isn't available in this game.".into())
    }

    pub fn state(&self, id: u64) -> Option<steam::ItemState> {
        let id = steam::ItemId::new(id).ok()?;
        self.steam.as_ref()?.item_state(id).ok()
    }

    pub fn loading(&self) -> bool {
        self.reading.is_some() || self.typed.is_some()
    }

    /// Shows a source, reading it when it has not been read yet.
    pub fn open(&mut self, source: Source, text: &str) -> Result<()> {
        self.source = source;
        self.reading = None;
        self.typed = None;
        self.problem = None;
        match source {
            Source::Subscribed => {
                let ids = self
                    .steam()?
                    .subscribed_items()
                    .map_err(|error| error.to_string())?;
                let known = ids.iter().collect::<BTreeSet<_>>();
                let read = self
                    .subscribed_items
                    .iter()
                    .map(|item| item.id)
                    .chain(
                        self.unavailable
                            .iter()
                            .filter(|(source, _)| *source == Source::Subscribed)
                            .filter_map(|(_, item)| item.id),
                    )
                    .collect::<BTreeSet<_>>();
                if ids.len() == read.len() && read.iter().all(|id| known.contains(id)) {
                    return Ok(());
                }
                self.subscribed = ids;
                self.subscribed_items.clear();
                self.unavailable
                    .retain(|(source, _)| *source != Source::Subscribed);
                self.read_subscribed(0)
            }
            Source::Everything if self.searched.as_deref() == Some(text) => Ok(()),
            Source::Everything => self.search(text, 1),
        }
    }

    fn read_subscribed(&mut self, first: usize) -> Result<()> {
        let last = (first + PAGE).min(self.subscribed.len());
        if first == last {
            return Ok(());
        }
        let query = self
            .steam()?
            .start_details(&self.subscribed[first..last], QUERY_TIMEOUT)
            .map_err(|error| error.to_string())?;
        self.reading = Some(Reading::Subscribed { query, next: last });
        Ok(())
    }

    fn search(&mut self, text: &str, page: u32) -> Result<()> {
        // Results are committed when Steam answers, so a dropped or failed
        // request leaves the shown page and its query as they were.
        if page == 1 {
            self.found.clear();
            self.unavailable
                .retain(|(source, _)| *source != Source::Everything);
            self.total = 0;
            self.page = 0;
            self.searched = None;
        }
        // What the player searches for is theirs, so only its shape is recorded.
        crate::log::debug(
            "workshop.search",
            format!(
                "Searching the Workshop for {} characters, page {page}",
                text.chars().count()
            ),
        )
        .write();
        let query = self
            .steam()?
            .start_search(text, page, QUERY_TIMEOUT)
            .map_err(|error| error.to_string())?;
        self.reading = Some(Reading::Search {
            query,
            page,
            text: text.to_owned(),
        });
        Ok(())
    }

    /// The search text changed. Only a Workshop-wide search has to ask Steam,
    /// and only for text whose results are not already shown.
    pub fn type_text(&mut self, text: &str) {
        if self.source != Source::Everything {
            return;
        }
        self.reading = None;
        self.typed =
            (self.searched.as_deref() != Some(text)).then(|| (text.to_owned(), Instant::now()));
    }

    pub fn has_more(&self) -> bool {
        self.source == Source::Everything
            && !self.loading()
            && (self.page as usize * PAGE) < self.total as usize
    }

    pub fn more(&mut self) -> Result<()> {
        if !self.has_more() {
            return Ok(());
        }
        let text = self.searched.clone().unwrap_or_default();
        self.search(&text, self.page + 1)
    }

    pub fn items(&self, text: &str) -> Vec<ItemView> {
        let filter = text.trim().to_lowercase();
        let items = match self.source {
            Source::Subscribed => &self.subscribed_items,
            Source::Everything => &self.found,
        };
        let mut views = items
            .iter()
            .map(|item| ItemView {
                id: Some(item.id.get()),
                title: item.title.clone(),
                description: item.description.clone(),
                updated: Some(item.updated_at),
                bytes: item.size_bytes,
                needs: item.children.len(),
                state: self.state(item.id.get()),
                unavailable: None,
                contains: Vec::new(),
            })
            .collect::<Vec<_>>();
        views.extend(
            self.unavailable
                .iter()
                .filter(|(source, _)| *source == self.source)
                .map(|(_, item)| ItemView {
                    id: item.id.map(steam::ItemId::get),
                    title: match item.id {
                        Some(id) => format!("Workshop item {}", id.get()),
                        None => "Unknown Workshop item".into(),
                    },
                    description: String::new(),
                    updated: None,
                    bytes: None,
                    needs: 0,
                    state: None,
                    unavailable: Some(item.reason.clone()),
                    contains: Vec::new(),
                }),
        );
        if self.source == Source::Subscribed {
            views.retain(|view| filter.is_empty() || view.matches(&filter));
            views.sort_by_key(|view| view.title.to_lowercase());
        }
        views
    }

    pub fn summary(&self, shown: usize) -> String {
        match self.source {
            Source::Subscribed => plural(shown, "subscribed item", "subscribed items"),
            Source::Everything if self.searched.is_none() => String::new(),
            Source::Everything => format!("{shown} of {} results", self.total),
        }
    }

    pub fn subscribe(&mut self, id: u64) -> Result<()> {
        self.refuse_second_delivery()?;
        let id = steam::ItemId::new(id).map_err(|error| error.to_string())?;
        let subscription = self
            .steam()?
            .subscribe(steam::UserIntent::subscribe(id), QUERY_TIMEOUT)
            .map_err(|error| error.to_string())?;
        self.delivery = Some(Delivery::Subscribe(subscription));
        Ok(())
    }

    pub fn download(&mut self, id: u64) -> Result<()> {
        self.refuse_second_delivery()?;
        self.start_download(id)
    }

    fn start_download(&mut self, id: u64) -> Result<()> {
        let id = steam::ItemId::new(id).map_err(|error| error.to_string())?;
        let download = self
            .steam()?
            .download(steam::UserIntent::download(id), DELIVERY_TIMEOUT)
            .map_err(|error| error.to_string())?;
        self.delivery = Some(Delivery::Download(download));
        Ok(())
    }

    fn refuse_second_delivery(&self) -> Result<()> {
        if self.delivery.is_some() {
            Err("Wait for the current download to finish.".into())
        } else {
            Ok(())
        }
    }

    pub fn poll(&mut self) -> Vec<Progress> {
        let mut progress = Vec::new();
        if let Some((text, at)) = &self.typed
            && at.elapsed() >= TYPING_PAUSE
        {
            let text = text.clone();
            self.typed = None;
            if let Err(error) = self.search(&text, 1) {
                self.fail(error, &mut progress);
            }
            progress.push(Progress::Items);
        }
        if let Err(error) = self.poll_reading(&mut progress) {
            self.reading = None;
            self.fail(error, &mut progress);
            progress.push(Progress::Items);
        }
        if let Err(error) = self.poll_delivery(&mut progress) {
            self.delivery = None;
            crate::log::error("workshop.delivery", error.clone()).write();
            progress.push(Progress::Failed(error));
            progress.push(Progress::Items);
        }
        progress
    }

    fn fail(&mut self, error: String, progress: &mut Vec<Progress>) {
        crate::log::warn("workshop.read", error.clone()).write();
        self.problem = Some(error.clone());
        progress.push(Progress::Failed(error));
    }

    fn poll_reading(&mut self, progress: &mut Vec<Progress>) -> Result<()> {
        let Some(reading) = &mut self.reading else {
            return Ok(());
        };
        let query = match reading {
            Reading::Subscribed { query, .. } | Reading::Search { query, .. } => query,
        };
        let steam::Poll::Ready(results) = query.poll().map_err(|error| error.to_string())? else {
            return Ok(());
        };
        let Some(reading) = self.reading.take() else {
            return Ok(());
        };
        let source = match reading {
            Reading::Subscribed { .. } => Source::Subscribed,
            Reading::Search { .. } => Source::Everything,
        };
        crate::log::debug("workshop.results", "Steam answered")
            .with(json!({
                "source": format!("{source:?}"),
                "items": results.items.len(),
                "unavailable": results.unavailable.len(),
                "total": results.total_matching,
                "cached": results.cached,
            }))
            .write();
        self.unavailable
            .extend(results.unavailable.into_iter().map(|item| (source, item)));
        match reading {
            Reading::Subscribed { next, .. } => {
                self.subscribed_items.extend(results.items);
                self.read_subscribed(next)?;
            }
            Reading::Search { page, text, .. } => {
                self.searched = Some(text);
                self.total = results.total_matching;
                self.page = page;
                self.found.extend(results.items);
            }
        }
        progress.push(Progress::Items);
        Ok(())
    }

    fn poll_delivery(&mut self, progress: &mut Vec<Progress>) -> Result<()> {
        match &mut self.delivery {
            None => Ok(()),
            Some(Delivery::Subscribe(subscription)) => {
                match subscription.poll().map_err(|error| error.to_string())? {
                    steam::Poll::Pending => {
                        progress.push(Progress::Status("Waiting for Steam to subscribe...".into()));
                    }
                    steam::Poll::Ready(_) => {
                        let id = subscription.item().get();
                        crate::log::info("workshop.subscribed", format!("Subscribed to {id}"))
                            .write();
                        self.delivery = None;
                        self.start_download(id)?;
                        progress.push(Progress::Items);
                    }
                }
                Ok(())
            }
            Some(Delivery::Download(download)) => {
                match download.poll().map_err(|error| error.to_string())? {
                    steam::Poll::Pending => {
                        let state = download.progress().map_err(|error| error.to_string())?;
                        progress.push(Progress::Status(match state {
                            steam::DownloadProgress {
                                downloaded_bytes: Some(done),
                                total_bytes: Some(total),
                                ..
                            } if total > 0 => {
                                format!("Downloading {} of {}...", size(done), size(total))
                            }
                            _ => "Waiting for Steam to start the download...".into(),
                        }));
                    }
                    steam::Poll::Ready(_) => {
                        let id = download.item().get();
                        crate::log::info("workshop.downloaded", format!("Downloaded {id}")).write();
                        self.delivery = None;
                        progress.push(Progress::Delivered(id));
                    }
                }
                Ok(())
            }
        }
    }
}
