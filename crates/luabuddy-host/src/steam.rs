//! Workshop access through the game's initialized Steam context.
//!
//! This adapter never initializes Steam or runs its callback queue. It owns only
//! query handles and polls its own API calls on the attaching game thread.

use jni::{
    JNIEnv, JavaVM,
    objects::{GlobalRef, JValue},
};
use libloading::Library;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::{CString, c_char, c_void};
use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

pub const PROJECT_ZOMBOID_APP_ID: u32 = 108_600;
const QUERY_CALLBACK: i32 = 3401;
const SUBSCRIBE_CALLBACK: i32 = 1313;
const UNSUBSCRIBE_CALLBACK: i32 = 1315;
const RESULT_OK: i32 = 1;
const MAX_RESULTS: u32 = 50;
const MAX_SUBSCRIPTIONS: u32 = 100_000;
const MAX_CHILDREN: u32 = 10_000;
const MAX_QUERIES: usize = 8;
const INVALID_QUERY: u64 = u64::MAX;
macro_rules! required_exports {
    ($($variant:ident => $name:literal),* $(,)?) => {
        enum Export {
            $($variant),*
        }
        pub(crate) const REQUIRED_EXPORTS: &[(&str, &[u8])] = &[
            $(($name, concat!($name, "\0").as_bytes())),*
        ];
    };
}

required_exports! {
    Ugc => "SteamAPI_SteamUGC_v021",
    Utils => "SteamAPI_SteamUtils_v010",
    AppId => "SteamAPI_ISteamUtils_GetAppID",
    User => "SteamAPI_GetHSteamUser",
    CreateQuery => "SteamAPI_ISteamUGC_CreateQueryAllUGCRequestPage",
    CreateDetails => "SteamAPI_ISteamUGC_CreateQueryUGCDetailsRequest",
    SetSearch => "SteamAPI_ISteamUGC_SetSearchText",
    SetLongDescription => "SteamAPI_ISteamUGC_SetReturnLongDescription",
    SetChildren => "SteamAPI_ISteamUGC_SetReturnChildren",
    SendQuery => "SteamAPI_ISteamUGC_SendQueryUGCRequest",
    ReleaseQuery => "SteamAPI_ISteamUGC_ReleaseQueryUGCRequest",
    QueryResult => "SteamAPI_ISteamUGC_GetQueryUGCResult",
    QueryChildren => "SteamAPI_ISteamUGC_GetQueryUGCChildren",
    ApiCompleted => "SteamAPI_ISteamUtils_IsAPICallCompleted",
    ApiResult => "SteamAPI_ISteamUtils_GetAPICallResult",
    ApiFailure => "SteamAPI_ISteamUtils_GetAPICallFailureReason",
    ItemState => "SteamAPI_ISteamUGC_GetItemState",
    Subscribe => "SteamAPI_ISteamUGC_SubscribeItem",
    Unsubscribe => "SteamAPI_ISteamUGC_UnsubscribeItem",
    DownloadInfo => "SteamAPI_ISteamUGC_GetItemDownloadInfo",
    InstallInfo => "SteamAPI_ISteamUGC_GetItemInstallInfo",
    SubscriptionCount => "SteamAPI_ISteamUGC_GetNumSubscribedItems",
    SubscribedItems => "SteamAPI_ISteamUGC_GetSubscribedItems",
}

static DOWNLOAD_OBSERVER: AtomicBool = AtomicBool::new(false);
static DOWNLOAD_NOTIFICATIONS: OnceLock<Mutex<BTreeMap<ItemId, (Instant, i32)>>> = OnceLock::new();

pub(crate) fn download_observer_ready() {
    DOWNLOAD_OBSERVER.store(true, Ordering::Release);
}

pub(crate) fn record_download_result(item: ItemId, result: i32) -> Result<()> {
    let now = Instant::now();
    let mut notifications = DOWNLOAD_NOTIFICATIONS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|_| SteamError("Workshop notification state is poisoned".into()))?;
    notifications.retain(|_, (at, _)| now.duration_since(*at) < Duration::from_secs(300));
    if notifications.len() >= 1024
        && !notifications.contains_key(&item)
        && let Some(oldest) = notifications
            .iter()
            .min_by_key(|(_, (at, _))| *at)
            .map(|(id, _)| *id)
    {
        notifications.remove(&oldest);
    }
    notifications.insert(item, (now, result));
    Ok(())
}

pub(crate) fn observed_download_result(item: ItemId, since: Instant) -> Result<Option<i32>> {
    let notifications = DOWNLOAD_NOTIFICATIONS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|_| SteamError("Workshop notification state is poisoned".into()))?;
    Ok(notifications
        .get(&item)
        .filter(|(at, _)| *at >= since)
        .map(|(_, result)| *result))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamError(pub String);

impl fmt::Display for SteamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SteamError {}

type Result<T> = std::result::Result<T, SteamError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ItemId(u64);

impl ItemId {
    pub fn new(value: u64) -> Result<Self> {
        if value == 0 || value == u64::MAX {
            return Err(SteamError(
                "Workshop item ID must be a valid nonzero ID".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemState {
    pub subscribed: bool,
    pub installed: bool,
    pub needs_update: bool,
    pub downloading: bool,
    pub download_pending: bool,
    pub disabled_locally: bool,
    pub raw: u32,
}

impl ItemState {
    fn from_bits(raw: u32) -> Self {
        Self {
            subscribed: raw & 1 != 0,
            installed: raw & 4 != 0,
            needs_update: raw & 8 != 0,
            downloading: raw & 16 != 0,
            download_pending: raw & 32 != 0,
            disabled_locally: raw & 64 != 0,
            raw,
        }
    }

    pub fn ready(self) -> bool {
        self.installed && !self.needs_update && !self.downloading && !self.download_pending
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopItem {
    pub id: ItemId,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub updated_at: u32,
    pub size_bytes: Option<u64>,
    pub children: Vec<ItemId>,
    pub state: ItemState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResults {
    pub items: Vec<WorkshopItem>,
    pub unavailable: Vec<UnavailableItem>,
    pub total_matching: u32,
    pub cached: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnavailableItem {
    pub index: u32,
    pub id: Option<ItemId>,
    pub result: Option<i32>,
    pub consumer_app: Option<u32>,
    pub banned: Option<bool>,
    pub reason: String,
}

enum QueryItem {
    Available(WorkshopItem),
    Unavailable(UnavailableItem),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll<T> {
    Pending,
    Ready(T),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadProgress {
    pub state: ItemState,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub install_folder: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntentAction {
    Subscribe,
    Unsubscribe,
    Download,
}

/// A particular user action, rather than permission inherited from downloaded content.
#[derive(Debug, Clone, Copy)]
pub struct UserIntent {
    item: ItemId,
    action: IntentAction,
}

impl UserIntent {
    pub fn subscribe(item: ItemId) -> Self {
        Self {
            item,
            action: IntentAction::Subscribe,
        }
    }

    pub fn unsubscribe(item: ItemId) -> Self {
        Self {
            item,
            action: IntentAction::Unsubscribe,
        }
    }

    pub fn download(item: ItemId) -> Self {
        Self {
            item,
            action: IntentAction::Download,
        }
    }
}

#[derive(Clone)]
pub struct SteamWorkshop {
    api: Rc<Api>,
    download_request: Rc<Result<DownloadRequester>>,
}

enum DownloadRequester {
    Host(HostDownload),
    #[cfg(test)]
    Test(fn(ItemId) -> Result<bool>),
}

impl DownloadRequester {
    fn request(&self, item: ItemId) -> Result<bool> {
        match self {
            Self::Host(host) => host.request(item),
            #[cfg(test)]
            Self::Test(request) => request(item),
        }
    }
}

pub(crate) struct HostDownload {
    vm: JavaVM,
    instance: GlobalRef,
    owner: ThreadId,
}

fn host_jni_error(error: jni::errors::Error) -> SteamError {
    SteamError(error.to_string())
}

fn host_jni_result<T>(env: &mut JNIEnv<'_>, result: Result<T>, stage: &str) -> Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            if env.exception_check().map_err(host_jni_error)? {
                env.exception_clear().map_err(host_jni_error)?;
            }
            Err(SteamError(format!("{stage}: {error}")))
        }
    }
}

impl HostDownload {
    pub(crate) fn bind(env: &mut JNIEnv<'_>) -> Result<Self> {
        let result = Self::bind_checked(env);
        host_jni_result(env, result, "Resolving the game's Workshop download route")
    }

    fn bind_checked(env: &mut JNIEnv<'_>) -> Result<Self> {
        let class = env
            .find_class("zombie/core/znet/SteamWorkshop")
            .map_err(host_jni_error)?;
        let loader = env
            .call_method(&class, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
            .and_then(|value| value.l())
            .map_err(host_jni_error)?;
        let system = env
            .call_static_method(
                "java/lang/ClassLoader",
                "getSystemClassLoader",
                "()Ljava/lang/ClassLoader;",
                &[],
            )
            .and_then(|value| value.l())
            .map_err(host_jni_error)?;
        if !env.is_same_object(loader, system).map_err(host_jni_error)? {
            return Err(SteamError(
                "Unapproved Workshop download classloader".into(),
            ));
        }
        let instance = env
            .get_static_field(&class, "instance", "Lzombie/core/znet/SteamWorkshop;")
            .and_then(|value| value.l())
            .map_err(host_jni_error)?;
        if instance.is_null() {
            return Err(SteamError(
                "The host Workshop instance is unavailable".into(),
            ));
        }
        let instance_class = env
            .call_method(&instance, "getClass", "()Ljava/lang/Class;", &[])
            .and_then(|value| value.l())
            .map_err(host_jni_error)?;
        if !env
            .is_same_object(&class, instance_class)
            .map_err(host_jni_error)?
        {
            return Err(SteamError(
                "Unexpected Workshop download instance class".into(),
            ));
        }
        let long_type = env
            .get_static_field("java/lang/Long", "TYPE", "Ljava/lang/Class;")
            .and_then(|value| value.l())
            .map_err(host_jni_error)?;
        let bool_type = env
            .get_static_field("java/lang/Boolean", "TYPE", "Ljava/lang/Class;")
            .and_then(|value| value.l())
            .map_err(host_jni_error)?;
        let parameters = env
            .new_object_array(2, "java/lang/Class", jni::objects::JObject::null())
            .map_err(host_jni_error)?;
        env.set_object_array_element(&parameters, 0, long_type)
            .map_err(host_jni_error)?;
        env.set_object_array_element(&parameters, 1, &bool_type)
            .map_err(host_jni_error)?;
        let name = env.new_string("n_DownloadItem").map_err(host_jni_error)?;
        let method = env
            .call_method(
                &class,
                "getDeclaredMethod",
                "(Ljava/lang/String;[Ljava/lang/Class;)Ljava/lang/reflect/Method;",
                &[JValue::Object(&name), JValue::Object(&parameters)],
            )
            .and_then(|value| value.l())
            .map_err(host_jni_error)?;
        let modifiers = env
            .call_method(&method, "getModifiers", "()I", &[])
            .and_then(|value| value.i())
            .map_err(host_jni_error)?;
        let returned = env
            .call_method(&method, "getReturnType", "()Ljava/lang/Class;", &[])
            .and_then(|value| value.l())
            .map_err(host_jni_error)?;
        if modifiers & 0x010a != 0x0102
            || !env
                .is_same_object(returned, bool_type)
                .map_err(host_jni_error)?
        {
            return Err(SteamError(
                "Expected private native instance method SteamWorkshop.n_DownloadItem(JZ)Z".into(),
            ));
        }
        Ok(Self {
            vm: env.get_java_vm().map_err(host_jni_error)?,
            instance: env.new_global_ref(instance).map_err(host_jni_error)?,
            owner: thread::current().id(),
        })
    }

    pub(crate) fn request(&self, item: ItemId) -> Result<bool> {
        if thread::current().id() != self.owner {
            return Err(SteamError(
                "Run native Workshop downloads on the attaching game thread".into(),
            ));
        }
        let mut env = self.vm.get_env().map_err(host_jni_error)?;
        let result = env
            .call_method(
                self.instance.as_obj(),
                "n_DownloadItem",
                "(JZ)Z",
                &[JValue::Long(item.get() as i64), JValue::Bool(0)],
            )
            .and_then(|value| value.z())
            .map_err(host_jni_error);
        host_jni_result(&mut env, result, "Requesting a native Workshop download")
    }
}

impl SteamWorkshop {
    /// Attaches only to an already loaded Linux x86_64 Steam library.
    ///
    /// The game must keep its Steam context initialized while this adapter and its
    /// handles exist. Invoke this on the game's owner thread after Steam startup.
    pub fn attach_existing(env: &mut JNIEnv<'_>, library: &Path) -> Result<Self> {
        let enabled = env
            .call_static_method(
                "zombie/core/znet/SteamUtils",
                "isSteamModeEnabled",
                "()Z",
                &[],
            )
            .and_then(|v| v.z())
            .map_err(|e| SteamError(format!("Checking native Steam mode: {e}")))?;
        if !enabled {
            return Err(SteamError(
                "Workshop is unavailable because the game is running without Steam".into(),
            ));
        }
        let library = open_existing(library)?;
        // SAFETY: `open_existing` obtains a reference to the host's already loaded
        // library. Exact flat-API symbols are resolved below and held by `Api`.
        let api = unsafe { Api::load(library)? };
        api.check_owner()?;
        let download_request = HostDownload::bind(env).map(DownloadRequester::Host);
        Ok(Self {
            api: Rc::new(api),
            download_request: Rc::new(download_request),
        })
    }

    pub fn start_search(&self, search: &str, page: u32, timeout: Duration) -> Result<Query> {
        self.api.check_owner()?;
        if page == 0 || page > 1_000 || search.len() > 1024 || timeout.is_zero() {
            return Err(SteamError(
                "Search requires page 1..1000, at most 1024 UTF-8 bytes, and a positive timeout"
                    .into(),
            ));
        }
        let search = CString::new(search)
            .map_err(|_| SteamError("Search text contains a NUL byte".into()))?;
        let sort = if search.as_bytes().is_empty() { 0 } else { 11 };
        // SAFETY: Valid owner-thread context and fixed C ABI. Both app IDs are
        // Project Zomboid; sort selects ranked votes or ranked text search.
        let handle = unsafe {
            (self.api.create_query)(
                self.api.ugc,
                sort,
                0,
                PROJECT_ZOMBOID_APP_ID,
                PROJECT_ZOMBOID_APP_ID,
                page,
            )
        };
        let mut query = Query::new(Rc::clone(&self.api), handle, timeout)?;
        // SAFETY: `query` owns a valid handle and the C string lives through this call.
        let configured = unsafe {
            (search.as_bytes().is_empty()
                || (self.api.set_search)(self.api.ugc, handle, search.as_ptr()))
                && (self.api.set_long_description)(self.api.ugc, handle, true)
                && (self.api.set_children)(self.api.ugc, handle, true)
        };
        if !configured {
            return Err(SteamError(
                "Steam rejected Workshop search parameters".into(),
            ));
        }
        // SAFETY: Configured query belongs to this initialized UGC interface.
        query.call = unsafe { (self.api.send_query)(self.api.ugc, handle) };
        if query.call == 0 {
            return Err(SteamError(
                "Steam did not accept the Workshop search request".into(),
            ));
        }
        Ok(query)
    }

    pub fn start_details(&self, items: &[ItemId], timeout: Duration) -> Result<Query> {
        self.api.check_owner()?;
        if items.is_empty() || items.len() > MAX_RESULTS as usize || timeout.is_zero() {
            return Err(SteamError(
                "Details query requires 1..50 item IDs and a positive timeout".into(),
            ));
        }
        let mut ids: Vec<u64> = items.iter().map(|id| id.get()).collect();
        // SAFETY: IDs are valid initialized u64 values; buffer length is at most50.
        let handle =
            unsafe { (self.api.create_details)(self.api.ugc, ids.as_mut_ptr(), ids.len() as u32) };
        let mut query = Query::new(Rc::clone(&self.api), handle, timeout)?;
        // SAFETY: Handle is owned by `query` and remains valid until release.
        let configured = unsafe {
            (self.api.set_long_description)(self.api.ugc, handle, true)
                && (self.api.set_children)(self.api.ugc, handle, true)
        };
        if !configured {
            return Err(SteamError(
                "Steam rejected Workshop details parameters".into(),
            ));
        }
        // SAFETY: Configured query belongs to this UGC context.
        query.call = unsafe { (self.api.send_query)(self.api.ugc, handle) };
        if query.call == 0 {
            return Err(SteamError(
                "Steam did not accept the Workshop details request".into(),
            ));
        }
        Ok(query)
    }

    pub fn subscribed_items(&self) -> Result<Vec<ItemId>> {
        self.api.check_owner()?;
        // SAFETY: Correct interface and owner thread; this call writes no buffers.
        let count = unsafe { (self.api.subscription_count)(self.api.ugc, true) };
        if count > MAX_SUBSCRIPTIONS {
            return Err(SteamError(
                "Steam subscription count exceeds the supported safety bound".into(),
            ));
        }
        let mut ids = vec![0; count as usize];
        // SAFETY: Steam receives the exact writable length of `ids`.
        let returned =
            unsafe { (self.api.subscribed_items)(self.api.ugc, ids.as_mut_ptr(), count, true) };
        if returned > count {
            return Err(SteamError(
                "Steam subscriptions changed during enumeration; retry the operation".into(),
            ));
        }
        ids.truncate(returned as usize);
        ids.into_iter().map(ItemId::new).collect()
    }

    pub fn item_state(&self, item: ItemId) -> Result<ItemState> {
        self.api.item_state(item)
    }

    pub fn download_progress(&self, item: ItemId) -> Result<DownloadProgress> {
        self.api.check_owner()?;
        let state = self.api.item_state(item)?;
        let mut downloaded = 0;
        let mut total = 0;
        // SAFETY: Outputs are distinct initialized u64 variables on this stack.
        let known = unsafe {
            (self.api.download_info)(self.api.ugc, item.get(), &mut downloaded, &mut total)
        };
        if known && downloaded > total {
            return Err(SteamError(
                "Steam reported download progress greater than its total".into(),
            ));
        }
        Ok(DownloadProgress {
            state,
            downloaded_bytes: known.then_some(downloaded),
            total_bytes: known.then_some(total),
            install_folder: self.api.install_folder(item)?,
        })
    }

    pub fn subscribe(&self, intent: UserIntent, timeout: Duration) -> Result<Subscription> {
        self.start_subscription(intent, IntentAction::Subscribe, timeout)
    }

    pub fn unsubscribe(&self, intent: UserIntent, timeout: Duration) -> Result<Subscription> {
        self.start_subscription(intent, IntentAction::Unsubscribe, timeout)
    }

    fn start_subscription(
        &self,
        intent: UserIntent,
        action: IntentAction,
        timeout: Duration,
    ) -> Result<Subscription> {
        self.api.check_owner()?;
        if intent.action != action || timeout.is_zero() {
            return Err(SteamError("Workshop subscription action requires matching explicit user intent and a positive timeout".into()));
        }
        let deadline = deadline(timeout)?;
        if self
            .api
            .pending_subscriptions
            .borrow()
            .contains_key(&intent.item)
        {
            return Err(SteamError(
                "A subscription change for this item is already pending".into(),
            ));
        }
        // SAFETY: Valid context, item ID, and explicit action. This is the only
        // account-changing boundary; no downloaded content invokes it implicitly.
        let call = unsafe {
            if action == IntentAction::Subscribe {
                (self.api.subscribe)(self.api.ugc, intent.item.get())
            } else {
                (self.api.unsubscribe)(self.api.ugc, intent.item.get())
            }
        };
        if call == 0 {
            return Err(SteamError(
                "Steam did not accept the subscription change".into(),
            ));
        }
        self.api
            .pending_subscriptions
            .borrow_mut()
            .insert(intent.item, call);
        Ok(Subscription {
            api: Rc::clone(&self.api),
            call,
            item: intent.item,
            subscribing: action == IntentAction::Subscribe,
            deadline,
            phase: SubscriptionPhase::AwaitingResult,
        })
    }

    pub fn download(&self, intent: UserIntent, timeout: Duration) -> Result<Download> {
        self.api.check_owner()?;
        if !DOWNLOAD_OBSERVER.load(Ordering::Acquire) {
            return Err(SteamError(
                "This game layout has no verified Workshop download-completion observer".into(),
            ));
        }
        if intent.action != IntentAction::Download || timeout.is_zero() {
            return Err(SteamError(
                "Downloading requires matching explicit user intent and a positive timeout".into(),
            ));
        }
        let deadline = deadline(timeout)?;
        if self
            .api
            .download_outcomes
            .borrow()
            .contains_key(&intent.item)
        {
            return Err(SteamError(
                "A download for this item is already pending".into(),
            ));
        }
        let started = Instant::now();
        let accepted = self
            .download_request
            .as_ref()
            .as_ref()
            .map_err(Clone::clone)?
            .request(intent.item)?;
        if !accepted {
            return Err(SteamError("The game's Workshop downloader refused this request; another host download may be active".into()));
        }
        self.api
            .download_outcomes
            .borrow_mut()
            .insert(intent.item, None);
        Ok(Download {
            workshop: self.clone(),
            item: intent.item,
            started,
            deadline,
            finished: false,
        })
    }

    /// Records the game's existing `onItemDownloaded`/`onItemNotDownloaded` dispatch.
    /// A successful result still needs installed-state verification in `Download::poll`.
    pub fn observe_download_result(&self, item: ItemId, result: i32) -> Result<()> {
        self.api.check_owner()?;
        if let Some(outcome) = self.api.download_outcomes.borrow_mut().get_mut(&item) {
            *outcome = Some(result);
        }
        Ok(())
    }

    pub fn cancel_download(&self, _item: ItemId) -> Result<()> {
        Err(SteamError("Steam's item API cannot cancel one download. Closing LuaBuddy stops observation; Steam may continue downloading".into()))
    }
}

pub struct Query {
    api: Rc<Api>,
    handle: Option<u64>,
    call: u64,
    deadline: Instant,
    finished: bool,
}

impl Query {
    fn new(api: Rc<Api>, handle: u64, timeout: Duration) -> Result<Self> {
        if handle == INVALID_QUERY || handle == 0 {
            return Err(SteamError(
                "Steam could not create the Workshop query".into(),
            ));
        }
        if api.active_queries.get() >= MAX_QUERIES {
            // SAFETY: The just-created query belongs to this context, and has not
            // been published or released elsewhere.
            unsafe { (api.release_query)(api.ugc, handle) };
            return Err(SteamError("Too many Workshop queries are pending; finish or cancel one before searching again".into()));
        }
        let deadline = match deadline(timeout) {
            Ok(deadline) => deadline,
            Err(error) => {
                // SAFETY: The just-created query is still exclusively owned here.
                unsafe { (api.release_query)(api.ugc, handle) };
                return Err(error);
            }
        };
        api.active_queries.set(api.active_queries.get() + 1);
        Ok(Self {
            api,
            handle: Some(handle),
            call: 0,
            deadline,
            finished: false,
        })
    }

    pub fn poll(&mut self) -> Result<Poll<SearchResults>> {
        self.api.check_owner()?;
        if self.finished {
            return Err(SteamError(
                "Workshop query has already completed or been cancelled".into(),
            ));
        }
        if Instant::now() >= self.deadline {
            self.cancel()?;
            return Err(SteamError(
                "Workshop query timed out. Check the Steam connection and retry".into(),
            ));
        }
        match self.api.call_completed(self.call) {
            Ok(false) => return Ok(Poll::Pending),
            Ok(true) => (),
            Err(error) => {
                self.cancel()?;
                return Err(error);
            }
        }
        let mut response = QueryCompleted {
            handle: 0,
            result: 0,
            returned: 0,
            total: 0,
            cached: 0,
            cursor: [0; 256],
        };
        // SAFETY: Linux callback3401 has packed4 size280, corroborated by the
        // installed host's CCallResult callback-size implementation.
        let completed = unsafe {
            self.api
                .call_result(self.call, QUERY_CALLBACK, &mut response)
        };
        if let Err(error) = completed {
            self.cancel()?;
            return Err(error);
        }
        let handle = self
            .handle
            .ok_or_else(|| SteamError("Workshop query handle was released".into()))?;
        if response.result != RESULT_OK
            || response.handle != handle
            || response.returned > MAX_RESULTS
            || response.cached > 1
        {
            self.cancel()?;
            return Err(SteamError(format!(
                "Workshop query failed with Steam result {} or incompatible response",
                response.result
            )));
        }
        let mut items = Vec::with_capacity(response.returned as usize);
        let mut unavailable = Vec::new();
        for index in 0..response.returned {
            match self.api.query_item(handle, index) {
                Ok(QueryItem::Available(item)) => items.push(item),
                Ok(QueryItem::Unavailable(item)) => unavailable.push(item),
                Err(error) => {
                    self.cancel()?;
                    return Err(error);
                }
            }
        }
        let result = SearchResults {
            items,
            unavailable,
            total_matching: response.total,
            cached: response.cached != 0,
        };
        self.cancel()?;
        Ok(Poll::Ready(result))
    }

    pub fn cancel(&mut self) -> Result<()> {
        self.api.check_owner()?;
        self.finished = true;
        if let Some(handle) = self.handle.take() {
            self.api
                .active_queries
                .set(self.api.active_queries.get() - 1);
            // SAFETY: The query exclusively owns this handle and releases it once.
            if !unsafe { (self.api.release_query)(self.api.ugc, handle) } {
                return Err(SteamError(
                    "Steam failed to release the Workshop query".into(),
                ));
            }
        }
        Ok(())
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.api
                .active_queries
                .set(self.api.active_queries.get() - 1);
            // SAFETY: Rc and the marker make Query neither Send nor Sync. The
            // host context outlives this query under attach_existing's contract.
            if self.api.check_owner().is_ok() {
                unsafe { (self.api.release_query)(self.api.ugc, handle) };
            }
        }
    }
}

pub struct Subscription {
    api: Rc<Api>,
    call: u64,
    item: ItemId,
    subscribing: bool,
    deadline: Instant,
    phase: SubscriptionPhase,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SubscriptionPhase {
    AwaitingResult,
    AwaitingState,
    Finished,
}

impl Subscription {
    pub fn poll(&mut self) -> Result<Poll<ItemState>> {
        if let Err(error) = self.api.check_owner() {
            self.phase = SubscriptionPhase::Finished;
            self.release_pending();
            return Err(error);
        }
        if self.phase == SubscriptionPhase::Finished {
            return Err(SteamError(
                "Subscription request has already completed".into(),
            ));
        }
        if Instant::now() >= self.deadline {
            self.phase = SubscriptionPhase::Finished;
            self.release_pending();
            return Err(SteamError("Subscription result is unconfirmed after timeout. Refresh item state before retrying; Steam may still finish the request".into()));
        }
        let outcome = self.poll_pending();
        if !matches!(&outcome, Ok(Poll::Pending)) {
            self.phase = SubscriptionPhase::Finished;
            self.release_pending();
        }
        outcome
    }

    fn poll_pending(&mut self) -> Result<Poll<ItemState>> {
        if self.phase == SubscriptionPhase::AwaitingResult {
            if !self.api.call_completed(self.call)? {
                return Ok(Poll::Pending);
            }
            let mut response = SubscriptionResult { result: 0, item: 0 };
            let callback = if self.subscribing {
                SUBSCRIBE_CALLBACK
            } else {
                UNSUBSCRIBE_CALLBACK
            };
            // SAFETY: RemoteStorage subscription result is packed4 size12 on Linux;
            // exact callback ID is selected for this request's action.
            unsafe { self.api.call_result(self.call, callback, &mut response) }?;
            if response.result != RESULT_OK || response.item != self.item.get() {
                return Err(SteamError(format!(
                    "Workshop subscription change failed with Steam result {}",
                    response.result
                )));
            }
            self.phase = SubscriptionPhase::AwaitingState;
        }
        let state = self.api.item_state(self.item)?;
        if state.subscribed != self.subscribing {
            return Ok(Poll::Pending);
        }
        Ok(Poll::Ready(state))
    }

    pub fn item(&self) -> ItemId {
        self.item
    }

    fn release_pending(&self) {
        let mut pending = self.api.pending_subscriptions.borrow_mut();
        if pending.get(&self.item) == Some(&self.call) {
            pending.remove(&self.item);
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.release_pending();
    }
}

pub struct Download {
    workshop: SteamWorkshop,
    item: ItemId,
    started: Instant,
    deadline: Instant,
    finished: bool,
}

impl Download {
    pub fn poll(&mut self) -> Result<Poll<DownloadProgress>> {
        if self.finished {
            return Err(SteamError(
                "Download observation has already completed".into(),
            ));
        }
        if let Some(result) = observed_download_result(self.item, self.started)? {
            self.workshop.observe_download_result(self.item, result)?;
        }
        let outcome = self
            .workshop
            .api
            .download_outcomes
            .borrow()
            .get(&self.item)
            .copied()
            .flatten();
        if let Some(result) = outcome
            && result != RESULT_OK
        {
            self.finished = true;
            return Err(SteamError(format!(
                "Workshop download failed with Steam result {result}"
            )));
        }
        let progress = self.workshop.download_progress(self.item)?;
        if outcome == Some(RESULT_OK) && progress.state.ready() && progress.install_folder.is_some()
        {
            self.finished = true;
            return Ok(Poll::Ready(progress));
        }
        if Instant::now() >= self.deadline {
            self.finished = true;
            return Err(SteamError("Workshop download remains unconfirmed after timeout. Steam may still be downloading; refresh its progress before retrying".into()));
        }
        Ok(Poll::Pending)
    }

    pub fn progress(&self) -> Result<DownloadProgress> {
        self.workshop.download_progress(self.item)
    }

    pub fn item(&self) -> ItemId {
        self.item
    }
}

impl Drop for Download {
    fn drop(&mut self) {
        self.workshop
            .api
            .download_outcomes
            .borrow_mut()
            .remove(&self.item);
    }
}

fn deadline(timeout: Duration) -> Result<Instant> {
    Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| SteamError("Workshop operation timeout exceeds supported range".into()))
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn open_existing(path: &Path) -> Result<Library> {
    const RTLD_NOLOAD: i32 = 4;
    if !path.is_absolute() {
        return Err(SteamError(
            "Steam library path must be the absolute path of the game's loaded library".into(),
        ));
    }
    // SAFETY: RTLD_NOLOAD does not load or run initialization from an unrelated
    // library. It only increments the reference of an already loaded object.
    let library = unsafe {
        libloading::os::unix::Library::open(
            Some(path),
            libloading::os::unix::RTLD_NOW | RTLD_NOLOAD,
        )
    }
    .map_err(|e| {
        SteamError(format!(
            "The game's Steam library is not already loaded: {e}"
        ))
    })?;
    Ok(library.into())
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn open_existing(_path: &Path) -> Result<Library> {
    Err(SteamError(
        "Native Workshop ABI has been verified only on Linux x86_64; this platform is not enabled"
            .into(),
    ))
}

type Interface = unsafe extern "C" fn() -> *mut c_void;
type AppId = unsafe extern "C" fn(*mut c_void) -> u32;
type SteamUser = unsafe extern "C" fn() -> i32;
type CreateQuery = unsafe extern "C" fn(*mut c_void, i32, i32, u32, u32, u32) -> u64;
type CreateDetails = unsafe extern "C" fn(*mut c_void, *mut u64, u32) -> u64;
type SetSearch = unsafe extern "C" fn(*mut c_void, u64, *const c_char) -> bool;
type SetFlag = unsafe extern "C" fn(*mut c_void, u64, bool) -> bool;
type SendQuery = unsafe extern "C" fn(*mut c_void, u64) -> u64;
type ReleaseQuery = unsafe extern "C" fn(*mut c_void, u64) -> bool;
type QueryResult = unsafe extern "C" fn(*mut c_void, u64, u32, *mut SteamDetails) -> bool;
type QueryChildren = unsafe extern "C" fn(*mut c_void, u64, u32, *mut u64, u32) -> bool;
type ApiCompleted = unsafe extern "C" fn(*mut c_void, u64, *mut bool) -> bool;
type ApiResult = unsafe extern "C" fn(*mut c_void, u64, *mut c_void, i32, i32, *mut bool) -> bool;
type ApiFailure = unsafe extern "C" fn(*mut c_void, u64) -> i32;
type ItemStateFn = unsafe extern "C" fn(*mut c_void, u64) -> u32;
type ItemAction = unsafe extern "C" fn(*mut c_void, u64) -> u64;
type DownloadInfo = unsafe extern "C" fn(*mut c_void, u64, *mut u64, *mut u64) -> bool;
type InstallInfo =
    unsafe extern "C" fn(*mut c_void, u64, *mut u64, *mut c_char, u32, *mut u32) -> bool;
type SubscriptionCount = unsafe extern "C" fn(*mut c_void, bool) -> u32;
type SubscribedItems = unsafe extern "C" fn(*mut c_void, *mut u64, u32, bool) -> u32;

struct Api {
    ugc: *mut c_void,
    utils: *mut c_void,
    owner: ThreadId,
    app_id: AppId,
    steam_user: SteamUser,
    ugc_getter: Interface,
    utils_getter: Interface,
    create_query: CreateQuery,
    create_details: CreateDetails,
    set_search: SetSearch,
    set_long_description: SetFlag,
    set_children: SetFlag,
    send_query: SendQuery,
    release_query: ReleaseQuery,
    query_result: QueryResult,
    query_children: QueryChildren,
    api_completed: ApiCompleted,
    api_result: ApiResult,
    api_failure: ApiFailure,
    item_state: ItemStateFn,
    subscribe: ItemAction,
    unsubscribe: ItemAction,
    download_info: DownloadInfo,
    install_info: InstallInfo,
    subscription_count: SubscriptionCount,
    subscribed_items: SubscribedItems,
    active_queries: Cell<usize>,
    pending_subscriptions: RefCell<BTreeMap<ItemId, u64>>,
    download_outcomes: RefCell<BTreeMap<ItemId, Option<i32>>>,
    _library: Library,
    _not_send: PhantomData<Rc<()>>,
}

impl Api {
    /// # Safety
    /// `library` must be the initialized host's Linux x86_64 Steamworks library.
    /// It must expose UGC021/Utils010 with the documented flat C signatures and
    /// retain its Steam context for this owner thread until all handles drop.
    unsafe fn load(library: Library) -> Result<Self> {
        macro_rules! symbol {
            ($export:ident, $ty:ty) => {{
                let (name, bytes) = REQUIRED_EXPORTS[Export::$export as usize];
                // SAFETY: Caller establishes the Steam library and supported ABI;
                // the shared declaration binds each export name to this typed lookup.
                unsafe { library.get::<$ty>(bytes) }
                    .map(|symbol| *symbol)
                    .map_err(|e| SteamError(format!("Missing Steam capability {name}: {e}")))?
            }};
        }
        let ugc_getter = symbol!(Ugc, Interface);
        let utils_getter = symbol!(Utils, Interface);
        // SAFETY: Accessors belong to the already initialized host library.
        let (ugc, utils) = unsafe { (ugc_getter(), utils_getter()) };
        if ugc.is_null() || utils.is_null() {
            return Err(SteamError(
                "The host Steam UGC/Utils context is not initialized".into(),
            ));
        }
        Ok(Self {
            ugc,
            utils,
            owner: thread::current().id(),
            app_id: symbol!(AppId, AppId),
            steam_user: symbol!(User, SteamUser),
            ugc_getter,
            utils_getter,
            create_query: symbol!(CreateQuery, CreateQuery),
            create_details: symbol!(CreateDetails, CreateDetails),
            set_search: symbol!(SetSearch, SetSearch),
            set_long_description: symbol!(SetLongDescription, SetFlag),
            set_children: symbol!(SetChildren, SetFlag),
            send_query: symbol!(SendQuery, SendQuery),
            release_query: symbol!(ReleaseQuery, ReleaseQuery),
            query_result: symbol!(QueryResult, QueryResult),
            query_children: symbol!(QueryChildren, QueryChildren),
            api_completed: symbol!(ApiCompleted, ApiCompleted),
            api_result: symbol!(ApiResult, ApiResult),
            api_failure: symbol!(ApiFailure, ApiFailure),
            item_state: symbol!(ItemState, ItemStateFn),
            subscribe: symbol!(Subscribe, ItemAction),
            unsubscribe: symbol!(Unsubscribe, ItemAction),
            download_info: symbol!(DownloadInfo, DownloadInfo),
            install_info: symbol!(InstallInfo, InstallInfo),
            subscription_count: symbol!(SubscriptionCount, SubscriptionCount),
            subscribed_items: symbol!(SubscribedItems, SubscribedItems),
            download_outcomes: RefCell::new(BTreeMap::new()),
            pending_subscriptions: RefCell::new(BTreeMap::new()),
            active_queries: Cell::new(0),
            _library: library,
            _not_send: PhantomData,
        })
    }

    fn check_owner(&self) -> Result<()> {
        if thread::current().id() != self.owner {
            return Err(SteamError(
                "Run Workshop operations on the attaching game thread".into(),
            ));
        }
        // SAFETY: Access is confined to the host owner thread; the host keeps this
        // context alive. The user handle is checked before dereferencing interfaces.
        let valid = unsafe {
            (self.steam_user)() != 0
                && (self.ugc_getter)() == self.ugc
                && (self.utils_getter)() == self.utils
                && (self.app_id)(self.utils) == PROJECT_ZOMBOID_APP_ID
        };
        if !valid {
            return Err(SteamError(
                "The Project Zomboid Steam context is inactive or belongs to another application"
                    .into(),
            ));
        }
        Ok(())
    }

    fn call_completed(&self, call: u64) -> Result<bool> {
        let mut failed = false;
        // SAFETY: This handle was returned by one of this adapter's own calls;
        // the bool output is writable for exactly this call.
        let completed = unsafe { (self.api_completed)(self.utils, call, &mut failed) };
        if failed {
            // SAFETY: The failed API call belongs to this same Utils context.
            let reason = unsafe { (self.api_failure)(self.utils, call) };
            return Err(SteamError(format!(
                "Steam API call failed with transport reason {reason}"
            )));
        }
        Ok(completed)
    }

    /// # Safety
    /// `T` must be the exact packed callback struct for `callback` in this ABI.
    /// Every field must accept all possible byte patterns and `value` must be live.
    unsafe fn call_result<T>(&self, call: u64, callback: i32, value: &mut T) -> Result<()> {
        let mut failed = false;
        // SAFETY: Caller binds callback to the exact output type and length.
        let accepted = unsafe {
            (self.api_result)(
                self.utils,
                call,
                (value as *mut T).cast(),
                size_of::<T>() as i32,
                callback,
                &mut failed,
            )
        };
        if !accepted || failed {
            return Err(SteamError(format!(
                "Steam could not read callback {callback} for this request"
            )));
        }
        Ok(())
    }

    fn item_state(&self, item: ItemId) -> Result<ItemState> {
        self.check_owner()?;
        // SAFETY: Owner context and bounded ItemId invariant hold.
        Ok(ItemState::from_bits(unsafe {
            (self.item_state)(self.ugc, item.get())
        }))
    }

    fn install_folder(&self, item: ItemId) -> Result<Option<PathBuf>> {
        let mut folder = [0_u8; 4096];
        let mut size = 0;
        let mut timestamp = 0;
        // SAFETY: Distinct initialized output buffers match their declared lengths.
        let installed = unsafe {
            (self.install_info)(
                self.ugc,
                item.get(),
                &mut size,
                folder.as_mut_ptr().cast(),
                folder.len() as u32,
                &mut timestamp,
            )
        };
        if !installed {
            return Ok(None);
        }
        let text = terminated_utf8(&folder, "Workshop install folder")?;
        if text.is_empty() {
            return Err(SteamError(
                "Steam returned an empty installed Workshop path".into(),
            ));
        }
        Ok(Some(PathBuf::from(text)))
    }

    fn query_item(&self, query: u64, index: u32) -> Result<QueryItem> {
        let mut details = SteamDetails::empty();
        // SAFETY: Exact packed4 UGC021 struct with initialized byte-valid fields.
        let accepted = unsafe { (self.query_result)(self.ugc, query, index, &mut details) };
        if let Some(unavailable) = unavailable_details(accepted, &details, index)? {
            return Ok(QueryItem::Unavailable(unavailable));
        }
        let id = ItemId::new(details.id)?;
        let title = terminated_utf8(&details.title, "Workshop title")?;
        let description = terminated_utf8(&details.description, "Workshop description")?;
        let tags = terminated_utf8(&details.tags, "Workshop tags")?
            .split(',')
            .filter(|tag| !tag.is_empty())
            .map(str::to_owned)
            .collect();
        if details.children > MAX_CHILDREN {
            return Err(SteamError(
                "Workshop dependency count exceeds the supported safety bound".into(),
            ));
        }
        let mut children = vec![0_u64; details.children as usize];
        if !children.is_empty() {
            // SAFETY: Exact writable u64 count, bounded by MAX_CHILDREN.
            let accepted = unsafe {
                (self.query_children)(
                    self.ugc,
                    query,
                    index,
                    children.as_mut_ptr(),
                    children.len() as u32,
                )
            };
            if !accepted {
                return Ok(QueryItem::Unavailable(UnavailableItem {
                    index,
                    id: Some(id),
                    result: Some(RESULT_OK),
                    consumer_app: Some(PROJECT_ZOMBOID_APP_ID),
                    banned: Some(false),
                    reason: format!(
                        "Steam could not read dependencies for Workshop item {}; its dependency metadata is unverified",
                        id.get()
                    ),
                }));
            }
        }
        Ok(QueryItem::Available(WorkshopItem {
            id,
            title,
            description,
            tags,
            updated_at: details.updated,
            size_bytes: if details.total_file_size != 0 {
                Some(details.total_file_size)
            } else {
                u64::try_from(details.file_size).ok()
            },
            children: children
                .into_iter()
                .map(ItemId::new)
                .collect::<Result<Vec<_>>>()?,
            state: self.item_state(id)?,
        }))
    }
}

fn unavailable_details(
    accepted: bool,
    details: &SteamDetails,
    index: u32,
) -> Result<Option<UnavailableItem>> {
    if !accepted {
        return Ok(Some(UnavailableItem {
            index,
            id: None,
            result: None,
            consumer_app: None,
            banned: None,
            reason: format!("Steam could not retrieve Workshop result {index}"),
        }));
    }
    let id = ItemId::new(details.id).ok();
    let result = details.result;
    if result != RESULT_OK {
        return Ok(Some(UnavailableItem {
            index,
            id,
            result: Some(result),
            consumer_app: None,
            banned: None,
            reason: format!(
                "Steam returned result {result} for Workshop result {index}; item details are unavailable"
            ),
        }));
    }
    if details.banned > 1 || details.accepted > 1 || details.truncated > 1 {
        return Err(SteamError(format!(
            "Workshop result {index} contains an invalid native boolean layout"
        )));
    }
    let app = details.consumer_app;
    let banned = details.banned != 0;
    let reason = if app != PROJECT_ZOMBOID_APP_ID {
        Some(format!(
            "Workshop result {index} belongs to application {app}, not Project Zomboid {PROJECT_ZOMBOID_APP_ID}"
        ))
    } else if banned {
        Some(format!("Workshop result {index} is banned by Steam"))
    } else {
        None
    };
    Ok(reason.map(|reason| UnavailableItem {
        index,
        id,
        result: Some(result),
        consumer_app: Some(app),
        banned: Some(banned),
        reason,
    }))
}

fn terminated_utf8(bytes: &[u8], field: &str) -> Result<String> {
    let end = bytes
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| SteamError(format!("{field} is not NUL-terminated")))?;
    String::from_utf8(bytes[..end].to_vec())
        .map_err(|_| SteamError(format!("{field} is not valid UTF-8")))
}

#[repr(C, packed(4))]
struct QueryCompleted {
    handle: u64,
    result: i32,
    returned: u32,
    total: u32,
    cached: u8,
    cursor: [u8; 256],
}

#[repr(C, packed(4))]
struct SubscriptionResult {
    result: i32,
    item: u64,
}

#[repr(C, packed(4))]
struct SteamDetails {
    id: u64,
    result: i32,
    file_type: i32,
    creator_app: u32,
    consumer_app: u32,
    title: [u8; 129],
    description: [u8; 8000],
    owner: u64,
    created: u32,
    updated: u32,
    added: u32,
    visibility: i32,
    banned: u8,
    accepted: u8,
    truncated: u8,
    tags: [u8; 1025],
    file: u64,
    preview: u64,
    filename: [u8; 260],
    file_size: i32,
    preview_size: i32,
    url: [u8; 256],
    votes_up: u32,
    votes_down: u32,
    score: f32,
    children: u32,
    total_file_size: u64,
}

impl SteamDetails {
    fn empty() -> Self {
        Self {
            id: 0,
            result: 0,
            file_type: 0,
            creator_app: 0,
            consumer_app: 0,
            title: [0; 129],
            description: [0; 8000],
            owner: 0,
            created: 0,
            updated: 0,
            added: 0,
            visibility: 0,
            banned: 0,
            accepted: 0,
            truncated: 0,
            tags: [0; 1025],
            file: 0,
            preview: 0,
            filename: [0; 260],
            file_size: 0,
            preview_size: 0,
            url: [0; 256],
            votes_up: 0,
            votes_down: 0,
            score: 0.0,
            children: 0,
            total_file_size: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn current_linux_abi_matches_host_callback_sizes_and_field_offsets() {
        assert_eq!(size_of::<QueryCompleted>(), 280);
        assert_eq!(align_of::<QueryCompleted>(), 4);
        assert_eq!(size_of::<SubscriptionResult>(), 12);
        assert_eq!(offset_of!(SubscriptionResult, item), 4);
        assert_eq!(size_of::<SteamDetails>(), 9772);
        assert_eq!(offset_of!(SteamDetails, title), 24);
        assert_eq!(offset_of!(SteamDetails, description), 153);
        assert_eq!(offset_of!(SteamDetails, created), 8164);
        assert_eq!(offset_of!(SteamDetails, file_size), 9484);
        assert_eq!(offset_of!(SteamDetails, children), 9760);
        assert_eq!(offset_of!(SteamDetails, total_file_size), 9764);
    }

    #[test]
    fn subscription_and_installed_content_are_distinct() {
        let subscribed = ItemState::from_bits(1);
        assert!(subscribed.subscribed);
        assert!(!subscribed.installed);
        assert!(!subscribed.ready());
        let installed = ItemState::from_bits(4);
        assert!(!installed.subscribed);
        assert!(installed.ready());
        assert!(!ItemState::from_bits(4 | 8).ready());
        assert!(!ItemState::from_bits(4 | 16).ready());
        assert!(!ItemState::from_bits(4 | 32).ready());
    }

    #[test]
    fn byte_buffers_reject_missing_terminator_and_invalid_utf8() {
        assert!(terminated_utf8(b"unterminated", "title").is_err());
        assert!(terminated_utf8(&[255, 0], "title").is_err());
        assert_eq!(
            terminated_utf8("日本語\0trailing".as_bytes(), "title").unwrap(),
            "日本語"
        );
    }

    #[test]
    fn item_identity_and_intent_are_action_scoped() {
        assert!(ItemId::new(0).is_err());
        assert!(ItemId::new(u64::MAX).is_err());
        let id = ItemId::new(3788245131).unwrap();
        assert_eq!(id.get(), 3788245131);
        assert_eq!(UserIntent::subscribe(id).action, IntentAction::Subscribe);
        assert_eq!(UserIntent::download(id).action, IntentAction::Download);
        assert_eq!(UserIntent::unsubscribe(id).item, id);
    }

    #[test]
    fn unavailable_details_retain_identity_and_result_without_interpreting_failed_metadata() {
        let mut details = SteamDetails::empty();
        details.id = 42;
        details.result = 15;
        details.banned = 255;
        details.accepted = 255;
        details.truncated = 255;
        let unavailable = unavailable_details(true, &details, 26).unwrap().unwrap();
        assert_eq!(unavailable.index, 26);
        assert_eq!(unavailable.id, Some(ItemId::new(42).unwrap()));
        assert_eq!(unavailable.result, Some(15));
        assert_eq!(unavailable.consumer_app, None);
        assert_eq!(unavailable.banned, None);
        assert!(unavailable.reason.contains("result 15"));
    }

    #[test]
    fn banned_and_foreign_items_are_rejected_individually() {
        let mut details = SteamDetails::empty();
        details.id = 42;
        details.result = RESULT_OK;
        details.consumer_app = PROJECT_ZOMBOID_APP_ID;
        details.banned = 1;
        let banned = unavailable_details(true, &details, 0).unwrap().unwrap();
        assert_eq!(banned.banned, Some(true));
        assert_eq!(banned.consumer_app, Some(PROJECT_ZOMBOID_APP_ID));
        details.banned = 0;
        details.consumer_app = 480;
        let foreign = unavailable_details(true, &details, 1).unwrap().unwrap();
        assert_eq!(foreign.consumer_app, Some(480));
        assert!(foreign.reason.contains("application 480"));
        details.consumer_app = PROJECT_ZOMBOID_APP_ID;
        assert_eq!(unavailable_details(true, &details, 2).unwrap(), None);
    }

    #[test]
    fn incompatible_success_layout_remains_a_query_failure() {
        let mut details = SteamDetails::empty();
        details.id = 42;
        details.result = RESULT_OK;
        details.consumer_app = PROJECT_ZOMBOID_APP_ID;
        details.banned = 2;
        assert!(unavailable_details(true, &details, 0).is_err());
    }

    #[test]
    fn rejected_native_result_exposes_no_unverified_fields() {
        let mut details = SteamDetails::empty();
        details.id = 42;
        details.result = RESULT_OK;
        details.banned = 255;
        let unavailable = unavailable_details(false, &details, 0).unwrap().unwrap();
        assert_eq!(unavailable.id, None);
        assert_eq!(unavailable.result, None);
        assert_eq!(unavailable.consumer_app, None);
        assert_eq!(unavailable.banned, None);
    }

    #[test]
    fn download_notifications_are_item_and_request_time_scoped() {
        let item = ItemId::new(42).unwrap();
        let started = Instant::now();
        record_download_result(item, 27).unwrap();
        assert_eq!(observed_download_result(item, started).unwrap(), Some(27));
        assert_eq!(
            observed_download_result(item, Instant::now() + Duration::from_secs(1)).unwrap(),
            None
        );
        assert_eq!(
            observed_download_result(ItemId::new(43).unwrap(), started).unwrap(),
            None
        );
    }

    #[test]
    fn deadline_overflow_is_a_recoverable_configuration_error() {
        assert!(deadline(Duration::MAX).is_err());
        assert!(deadline(Duration::from_secs(10)).is_ok());
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn native_attachment_never_loads_a_missing_library() {
        assert!(open_existing(Path::new("relative.so")).is_err());
        assert!(open_existing(Path::new("/luabuddy-nonexistent-library-for-test.so")).is_err());
    }
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
mod subscription_tests {
    use super::*;

    struct NativeState {
        active: bool,
        completed: bool,
        transport_failed: bool,
        result_accepted: bool,
        result_code: i32,
        response_item: u64,
        subscribed: bool,
        item_bits: u32,
        host_accepts: bool,
        host_error: bool,
        host_requests: Vec<u64>,
        folder: Option<&'static str>,
        actions: Vec<(bool, u64)>,
        result_reads: usize,
        unexpected_calls: usize,
    }

    impl Default for NativeState {
        fn default() -> Self {
            Self {
                active: true,
                completed: false,
                transport_failed: false,
                result_accepted: true,
                result_code: 1,
                response_item: 42,
                subscribed: false,
                item_bits: 0,
                host_accepts: true,
                host_error: false,
                host_requests: Vec::new(),
                folder: None,
                actions: Vec::new(),
                result_reads: 0,
                unexpected_calls: 0,
            }
        }
    }

    thread_local! {
        static NATIVE: RefCell<NativeState> = RefCell::new(NativeState::default());
    }

    extern "C" fn interface() -> *mut c_void {
        std::ptr::dangling_mut::<u8>().cast()
    }

    extern "C" fn app_id(_: *mut c_void) -> u32 {
        108_600
    }

    extern "C" fn steam_user() -> i32 {
        NATIVE.with(|s| i32::from(s.borrow().active))
    }

    /// # Safety
    /// A non-null `failed` must be the adapter's writable bool output.
    unsafe extern "C" fn completed(_: *mut c_void, _: u64, failed: *mut bool) -> bool {
        NATIVE.with(|s| {
            let state = s.borrow();
            if !failed.is_null() {
                // SAFETY: The adapter supplies its writable bool output.
                unsafe { failed.write(state.transport_failed) };
            }
            state.completed
        })
    }

    /// # Safety
    /// The adapter supplies disjoint writable callback and bool outputs.
    /// The callback buffer holds the declared packed subscription result.
    unsafe extern "C" fn result(
        _: *mut c_void,
        _: u64,
        value: *mut c_void,
        size: i32,
        callback: i32,
        failed: *mut bool,
    ) -> bool {
        NATIVE.with(|s| {
            let mut state = s.borrow_mut();
            state.result_reads += 1;
            let expected = if state.actions.last().is_some_and(|a| a.0) {
                1313
            } else {
                1315
            };
            if value.is_null() || failed.is_null() || size != 12 || callback != expected {
                return false;
            }
            // SAFETY: The checked output is the adapter's packed subscription result.
            unsafe {
                failed.write(false);
                value
                    .cast::<SubscriptionResult>()
                    .write_unaligned(SubscriptionResult {
                        result: state.result_code,
                        item: state.response_item,
                    });
            }
            state.result_accepted
        })
    }

    extern "C" fn failure(_: *mut c_void, _: u64) -> i32 {
        2
    }

    extern "C" fn item_state(_: *mut c_void, _: u64) -> u32 {
        NATIVE.with(|s| {
            let state = s.borrow();
            u32::from(state.subscribed) | state.item_bits
        })
    }

    fn action(subscribing: bool, item: u64) -> u64 {
        NATIVE.with(|s| {
            let mut state = s.borrow_mut();
            state.actions.push((subscribing, item));
            state.actions.len() as u64
        })
    }

    extern "C" fn subscribe(_: *mut c_void, item: u64) -> u64 {
        action(true, item)
    }

    extern "C" fn unsubscribe(_: *mut c_void, item: u64) -> u64 {
        action(false, item)
    }

    fn request_download(item: ItemId) -> Result<bool> {
        NATIVE.with(|s| {
            let mut state = s.borrow_mut();
            state.host_requests.push(item.get());
            if state.host_error {
                Err(SteamError("Host JNI download request failed".into()))
            } else {
                Ok(state.host_accepts)
            }
        })
    }

    extern "C" fn download_info(_: *mut c_void, _: u64, _: *mut u64, _: *mut u64) -> bool {
        false
    }

    /// # Safety
    /// Outputs are the adapter's disjoint buffers with the supplied capacity.
    unsafe extern "C" fn install_info(
        _: *mut c_void,
        _: u64,
        size: *mut u64,
        folder: *mut c_char,
        capacity: u32,
        timestamp: *mut u32,
    ) -> bool {
        NATIVE.with(|s| {
            let state = s.borrow();
            let Some(path) = state.folder else {
                return false;
            };
            if size.is_null()
                || folder.is_null()
                || timestamp.is_null()
                || capacity as usize <= path.len()
            {
                return false;
            }
            // SAFETY: The caller supplies writable outputs; the path fits in capacity.
            unsafe {
                size.write(4);
                timestamp.write(1);
                std::ptr::copy_nonoverlapping(path.as_ptr(), folder.cast(), path.len());
                folder.add(path.len()).write(0);
            }
            true
        })
    }

    macro_rules! unused {
        ($name:ident($($arg:ident: $ty:ty),* $(,)?) -> $output:ty, $value:expr) => {
            extern "C" fn $name($($arg: $ty),*) -> $output {
                $(let _ = $arg;)*
                NATIVE.with(|s| s.borrow_mut().unexpected_calls += 1);
                $value
            }
        };
    }

    unused!(
        query(a: *mut c_void, b: i32, c: i32, d: u32, e: u32, f: u32) -> u64,
        0
    );
    unused!(details(a: *mut c_void, b: *mut u64, c: u32) -> u64, 0);
    unused!(
        search(a: *mut c_void, b: u64, c: *const c_char) -> bool,
        false
    );
    unused!(flag(a: *mut c_void, b: u64, c: bool) -> bool, false);
    unused!(send(a: *mut c_void, b: u64) -> u64, 0);
    unused!(release(a: *mut c_void, b: u64) -> bool, false);
    unused!(
        query_result(a: *mut c_void, b: u64, c: u32, d: *mut SteamDetails) -> bool,
        false
    );
    unused!(
        children(a: *mut c_void, b: u64, c: u32, d: *mut u64, e: u32) -> bool,
        false
    );
    unused!(subscription_count(a: *mut c_void, b: bool) -> u32, 0);
    unused!(
        subscribed_items(a: *mut c_void, b: *mut u64, c: u32, d: bool) -> u32,
        0
    );

    fn workshop(subscribed: bool) -> SteamWorkshop {
        NATIVE.with(|s| {
            *s.borrow_mut() = NativeState {
                subscribed,
                ..NativeState::default()
            }
        });
        SteamWorkshop {
            api: Rc::new(Api {
                ugc: std::ptr::dangling_mut::<u8>().cast(),
                utils: std::ptr::dangling_mut::<u8>().cast(),
                owner: thread::current().id(),
                app_id,
                steam_user,
                ugc_getter: interface,
                utils_getter: interface,
                create_query: query,
                create_details: details,
                set_search: search,
                set_long_description: flag,
                set_children: flag,
                send_query: send,
                release_query: release,
                query_result,
                query_children: children,
                api_completed: completed,
                api_result: result,
                api_failure: failure,
                item_state,
                subscribe,
                unsubscribe,
                download_info,
                install_info,
                subscription_count,
                subscribed_items,
                active_queries: Cell::new(0),
                pending_subscriptions: RefCell::new(BTreeMap::new()),
                download_outcomes: RefCell::new(BTreeMap::new()),
                _library: libloading::os::unix::Library::this().into(),
                _not_send: PhantomData,
            }),
            download_request: Rc::new(Err(SteamError(
                "No download requester in subscription fixture".into(),
            ))),
        }
    }

    fn start(workshop: &SteamWorkshop, subscribing: bool, timeout: Duration) -> Subscription {
        let item = ItemId::new(42).unwrap();
        if subscribing {
            workshop.subscribe(UserIntent::subscribe(item), timeout)
        } else {
            workshop.unsubscribe(UserIntent::unsubscribe(item), timeout)
        }
        .unwrap()
    }

    fn downloading_workshop() -> SteamWorkshop {
        DOWNLOAD_OBSERVER.store(true, Ordering::Release);
        let mut workshop = workshop(true);
        workshop.download_request = Rc::new(Ok(DownloadRequester::Test(request_download)));
        workshop
    }

    #[test]
    fn host_download_request_requires_fresh_success_and_verified_installation() {
        let workshop = downloading_workshop();
        let item = ItemId::new(900_001).unwrap();
        record_download_result(item, RESULT_OK).unwrap();
        let mut request = workshop
            .download(UserIntent::download(item), Duration::from_secs(10))
            .unwrap();
        assert!(
            workshop
                .download(UserIntent::download(item), Duration::from_secs(10))
                .is_err()
        );
        NATIVE.with(|s| {
            let mut state = s.borrow_mut();
            state.item_bits = 4;
            state.folder = Some("/tmp/luabuddy-host-download-fixture");
        });
        assert_eq!(request.poll().unwrap(), Poll::Pending);
        record_download_result(item, RESULT_OK).unwrap();
        NATIVE.with(|s| s.borrow_mut().folder = None);
        assert_eq!(request.poll().unwrap(), Poll::Pending);
        NATIVE.with(|s| {
            let mut state = s.borrow_mut();
            state.item_bits = 4 | 16;
            state.folder = Some("/tmp/luabuddy-host-download-fixture");
        });
        assert_eq!(request.poll().unwrap(), Poll::Pending);
        NATIVE.with(|s| s.borrow_mut().item_bits = 4);
        let Poll::Ready(progress) = request.poll().unwrap() else {
            panic!("Fresh successful callback and installed path must complete the request");
        };
        assert_eq!(
            progress.install_folder,
            Some(PathBuf::from("/tmp/luabuddy-host-download-fixture"))
        );
        assert!(request.poll().is_err());
        NATIVE.with(|s| {
            let state = s.borrow();
            assert_eq!(state.host_requests, [900_001]);
            assert!(state.actions.is_empty());
        });
    }

    #[test]
    fn rejected_and_failed_host_downloads_do_not_create_an_observation() {
        let workshop = downloading_workshop();
        let item = ItemId::new(900_002).unwrap();
        NATIVE.with(|s| s.borrow_mut().host_accepts = false);
        assert!(
            workshop
                .download(UserIntent::download(item), Duration::from_secs(1))
                .is_err()
        );
        assert!(workshop.api.download_outcomes.borrow().is_empty());
        NATIVE.with(|s| s.borrow_mut().host_error = true);
        assert!(
            workshop
                .download(UserIntent::download(item), Duration::from_secs(1))
                .is_err()
        );
        assert!(workshop.api.download_outcomes.borrow().is_empty());
        NATIVE.with(|s| {
            let mut state = s.borrow_mut();
            state.host_accepts = true;
            state.host_error = false;
        });
        let mut request = workshop
            .download(UserIntent::download(item), Duration::from_secs(1))
            .unwrap();
        record_download_result(item, 27).unwrap();
        assert!(request.poll().unwrap_err().0.contains("27"));
        drop(request);
        let mut request = workshop
            .download(UserIntent::download(item), Duration::from_secs(1))
            .unwrap();
        request.deadline = Instant::now();
        assert!(request.poll().unwrap_err().0.contains("unconfirmed"));
        drop(request);
        assert!(workshop.api.download_outcomes.borrow().is_empty());
        assert!(
            workshop
                .cancel_download(item)
                .unwrap_err()
                .0
                .contains("cannot cancel")
        );
        NATIVE.with(|s| {
            let state = s.borrow();
            assert_eq!(state.host_requests, [900_002, 900_002, 900_002, 900_002]);
            assert!(state.actions.is_empty());
        });
    }

    #[test]
    fn acknowledged_changes_wait_for_state_without_repeating_the_native_request_or_result() {
        for subscribing in [true, false] {
            let workshop = workshop(!subscribing);
            let item = ItemId::new(42).unwrap();
            let mut request = start(&workshop, subscribing, Duration::from_secs(10));
            assert_eq!(request.poll().unwrap(), Poll::Pending);
            NATIVE.with(|s| s.borrow_mut().completed = true);
            assert_eq!(request.poll().unwrap(), Poll::Pending);
            assert_eq!(request.poll().unwrap(), Poll::Pending);
            assert!(
                workshop
                    .subscribe(UserIntent::subscribe(item), Duration::from_secs(1))
                    .is_err()
            );
            assert!(
                workshop
                    .unsubscribe(UserIntent::unsubscribe(item), Duration::from_secs(1))
                    .is_err()
            );
            NATIVE.with(|s| {
                let mut state = s.borrow_mut();
                assert_eq!(state.actions, [(subscribing, 42)]);
                assert_eq!(state.result_reads, 1);
                assert_eq!(state.unexpected_calls, 0);
                state.subscribed = subscribing;
            });
            let Poll::Ready(state) = request.poll().unwrap() else {
                panic!("Confirmed requested state must complete the operation");
            };
            assert_eq!(state.subscribed, subscribing);
            assert!(request.poll().is_err());
            let replacement = start(&workshop, subscribing, Duration::from_secs(1));
            drop(request);
            assert!(
                workshop
                    .subscribe(UserIntent::subscribe(item), Duration::from_secs(1))
                    .is_err()
            );
            drop(replacement);
            NATIVE.with(|s| {
                let state = s.borrow();
                assert_eq!(state.actions.len(), 2);
                assert_eq!(state.result_reads, 1);
                assert_eq!(state.unexpected_calls, 0);
            });
        }
    }

    #[test]
    fn acknowledged_change_keeps_the_original_deadline_and_releases_on_timeout() {
        let workshop = workshop(false);
        let mut request = start(&workshop, true, Duration::from_secs(10));
        let original_deadline = request.deadline;
        NATIVE.with(|s| s.borrow_mut().completed = true);
        assert_eq!(request.poll().unwrap(), Poll::Pending);
        assert_eq!(request.deadline, original_deadline);
        request.deadline = Instant::now();
        assert!(request.poll().unwrap_err().0.contains("unconfirmed"));
        assert!(request.poll().is_err());
        drop(start(&workshop, true, Duration::from_secs(1)));
        NATIVE.with(|s| {
            let state = s.borrow();
            assert_eq!(state.actions.len(), 2);
            assert_eq!(state.result_reads, 1);
            assert_eq!(state.unexpected_calls, 0);
        });
    }

    #[test]
    fn subscription_errors_release_ownership_without_repeating_account_writes() {
        for failure in ["transport", "read", "result", "identity", "context"] {
            let workshop = workshop(false);
            let mut request = start(&workshop, true, Duration::from_secs(10));
            NATIVE.with(|s| {
                let mut state = s.borrow_mut();
                state.completed = true;
                match failure {
                    "transport" => state.transport_failed = true,
                    "read" => state.result_accepted = false,
                    "result" => state.result_code = 15,
                    "identity" => state.response_item = 43,
                    "context" => state.active = false,
                    _ => unreachable!(),
                }
            });
            assert!(request.poll().is_err(), "{failure}");
            NATIVE.with(|s| s.borrow_mut().active = true);
            assert!(request.poll().is_err(), "{failure}");
            drop(start(&workshop, true, Duration::from_secs(1)));
            NATIVE.with(|s| {
                let state = s.borrow();
                assert_eq!(state.actions.len(), 2, "{failure}");
                assert_eq!(state.unexpected_calls, 0, "{failure}");
            });
        }
    }

    #[test]
    fn dropping_an_acknowledged_change_releases_ownership_without_cancelling_steam() {
        let workshop = workshop(false);
        let mut request = start(&workshop, true, Duration::from_secs(10));
        NATIVE.with(|s| s.borrow_mut().completed = true);
        assert_eq!(request.poll().unwrap(), Poll::Pending);
        drop(request);
        drop(start(&workshop, true, Duration::from_secs(1)));
        NATIVE.with(|s| {
            let state = s.borrow();
            assert_eq!(state.actions, [(true, 42), (true, 42)]);
            assert_eq!(state.result_reads, 1);
            assert_eq!(state.unexpected_calls, 0);
        });
    }
}
