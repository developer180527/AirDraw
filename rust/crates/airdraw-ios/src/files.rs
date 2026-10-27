//! System document picker and security-scoped access for files outside the app's container
//! (iCloud Drive, other apps' folders). Results are queued for the app to poll each frame.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_foundation::{
    NSArray, NSData, NSFileManager, NSSearchPathDirectory, NSSearchPathDomainMask, NSSet, NSString, NSURL,
    NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions,
};
use objc2_ui_kit::{UIApplication, UIDocumentPickerDelegate, UIDocumentPickerViewController, UIOpenURLContext, UIScene};
use objc2_uniform_type_identifiers::UTType;

use crate::FileEvent;

thread_local! {
    static EVENTS: RefCell<Vec<FileEvent>> = const { RefCell::new(Vec::new()) };
    /// URLs we started security-scoped access for, by path.
    static ACCESSING: RefCell<Vec<(String, Retained<NSURL>)>> = const { RefCell::new(Vec::new()) };
    /// The delegate is held weakly by the picker; keep it alive while a picker is shown.
    static DELEGATE: RefCell<Option<Retained<PickerDelegate>>> = const { RefCell::new(None) };
    static OPEN_SERVICE: RefCell<Option<Retained<OpenURLService>>> = const { RefCell::new(None) };
}

define_class!(
    /// Registered with Godot's app delegate to receive documents opened from other apps
    /// (Files, share sheets) while AirDraw runs.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AirDrawOpenURLService"]
    struct OpenURLService;

    unsafe impl NSObjectProtocol for OpenURLService {}

    impl OpenURLService {
        #[unsafe(method(scene:openURLContexts:))]
        fn open_url_contexts(&self, _scene: &UIScene, contexts: &NSSet<UIOpenURLContext>) {
            handle_contexts(contexts);
        }
    }
);

fn handle_contexts(contexts: &NSSet<UIOpenURLContext>) {
    for ctx in contexts.iter() {
        let url = ctx.URL();
        if let Some(path) = begin_access(&url) {
            let bookmark = bookmark_hex(&url);
            EVENTS.with(|e| e.borrow_mut().push(FileEvent::Picked { path, bookmark }));
        }
    }
}

/// Starts receiving documents opened from other apps, and picks up any document the app was
/// launched with (stashed by AirDraw's engine patch in `GDTApplicationDelegate`).
pub fn listen_for_opened_files() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    if OPEN_SERVICE.with(|s| s.borrow().is_some()) {
        return;
    }
    let Some(delegate_class) = AnyClass::get(c"GDTApplicationDelegate") else { return };
    let service: Retained<OpenURLService> = unsafe { msg_send![super(OpenURLService::alloc(mtm).set_ivars(())), init] };
    unsafe {
        let _: () = msg_send![delegate_class, addService: &*service];
        let launch: Option<Retained<NSSet<UIOpenURLContext>>> = msg_send![delegate_class, launchURLContexts];
        if let Some(launch) = launch {
            handle_contexts(&launch);
            let _: () = msg_send![delegate_class, clearLaunchURLContexts];
        }
    }
    OPEN_SERVICE.with(|s| *s.borrow_mut() = Some(service));
}

/// The app's Documents folder: what the Files app shows as "On My iPad › AirDraw".
pub fn documents_dir() -> Option<String> {
    let urls = NSFileManager::defaultManager()
        .URLsForDirectory_inDomains(NSSearchPathDirectory::DocumentDirectory, NSSearchPathDomainMask::UserDomainMask);
    urls.firstObject()?.path().map(|p| p.to_string())
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AirDrawPickerDelegate"]
    struct PickerDelegate;

    unsafe impl NSObjectProtocol for PickerDelegate {}

    unsafe impl UIDocumentPickerDelegate for PickerDelegate {
        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn did_pick(&self, _picker: &UIDocumentPickerViewController, urls: &NSArray<NSURL>) {
            if let Some(url) = urls.firstObject() {
                let event = match begin_access(&url) {
                    Some(path) => FileEvent::Picked { path, bookmark: bookmark_hex(&url) },
                    None => FileEvent::Cancelled,
                };
                EVENTS.with(|e| e.borrow_mut().push(event));
            }
            DELEGATE.with(|d| d.borrow_mut().take());
        }

        #[unsafe(method(documentPickerWasCancelled:))]
        fn cancelled(&self, _picker: &UIDocumentPickerViewController) {
            EVENTS.with(|e| e.borrow_mut().push(FileEvent::Cancelled));
            DELEGATE.with(|d| d.borrow_mut().take());
        }
    }
);

/// Shows the system picker for sketches. Returns false if it couldn't be shown.
pub fn pick(type_identifier: &str) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let app = UIApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    let Some(root) = app.keyWindow().or_else(|| app.windows().firstObject()).and_then(|w| w.rootViewController()) else {
        return false;
    };
    // Fall back to any data if the type isn't registered (e.g. an old install).
    let ty = UTType::typeWithIdentifier(&NSString::from_str(type_identifier))
        .or_else(|| UTType::typeWithIdentifier(&NSString::from_str("public.data")));
    let Some(ty) = ty else { return false };
    let picker = UIDocumentPickerViewController::initForOpeningContentTypes_asCopy(
        UIDocumentPickerViewController::alloc(mtm),
        &NSArray::from_retained_slice(&[ty]),
        false,
    );
    let delegate = PickerDelegate::alloc(mtm).set_ivars(());
    let delegate: Retained<PickerDelegate> = unsafe { msg_send![super(delegate), init] };
    picker.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    picker.setShouldShowFileExtensions(true);
    DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
    root.presentViewController_animated_completion(&picker, true, None);
    true
}

pub fn drain(out: &mut Vec<FileEvent>) {
    EVENTS.with(|e| out.append(&mut e.borrow_mut()));
}

/// Restores access from a bookmark made at pick time. Returns the file's current path.
pub fn open_bookmark(hex: &str) -> Option<String> {
    let bytes = from_hex(hex)?;
    let data = NSData::with_bytes(&bytes);
    let mut stale = objc2::runtime::Bool::NO;
    let url = unsafe {
        NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
            &data,
            NSURLBookmarkResolutionOptions(0),
            None,
            &mut stale,
        )
    }
    .ok()?;
    begin_access(&url)
}

/// Ends security-scoped access to a file (when its document closes).
pub fn release(path: &str) {
    ACCESSING.with(|a| {
        a.borrow_mut().retain(|(p, url)| {
            if p == path {
                unsafe { url.stopAccessingSecurityScopedResource() };
                false
            } else {
                true
            }
        })
    });
}

fn begin_access(url: &NSURL) -> Option<String> {
    let path = url.path()?.to_string();
    // Files inside our own container need no scope; this returns false for them.
    if unsafe { url.startAccessingSecurityScopedResource() } {
        ACCESSING.with(|a| a.borrow_mut().push((path.clone(), url.retain())));
    }
    Some(path)
}

fn bookmark_hex(url: &NSURL) -> Option<String> {
    let data = url.bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
        NSURLBookmarkCreationOptions(0),
        None,
        None,
    );
    data.ok().map(|d| d.to_vec().iter().map(|b| format!("{b:02x}")).collect())
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}
