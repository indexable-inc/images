//! Menu-bar volume for the local listener.
//!
//! macOS gets a real `NSStatusItem`; other platforms get the CLI pointer
//! (volume is equally reachable via `shared-audio volume ...` everywhere,
//! which is what desktop-Linux keybindings should call).

#[cfg(target_os = "macos")]
pub fn run() -> anyhow::Result<()> {
    macos::run()
}

#[cfg(not(target_os = "macos"))]
pub fn run() -> anyhow::Result<()> {
    anyhow::bail!(
        "the menu-bar tray is macOS-only; bind `shared-audio volume up|down|mute` \
         to media keys instead"
    )
}

#[cfg(target_os = "macos")]
mod macos {
    use anyhow::Result;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSMenu, NSMenuItem, NSStatusBar,
        NSStatusItem, NSVariableStatusItemLength,
    };
    use objc2_foundation::{NSObject, NSObjectProtocol, NSString, ns_string};

    use crate::client;
    use crate::control::Request;

    fn send_volume(set: Option<f32>, step: Option<f32>, muted: Option<bool>) {
        if let Err(error) = client::request(&Request::Volume { set, step, muted }) {
            eprintln!("shared-audio tray: {error:#}");
        }
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "SharedAudioTray"]
        struct Tray;

        unsafe impl NSObjectProtocol for Tray {}

        impl Tray {
            #[unsafe(method(volumeUp:))]
            fn volume_up(&self, _sender: Option<&AnyObject>) {
                send_volume(None, Some(0.1), None);
            }

            #[unsafe(method(volumeDown:))]
            fn volume_down(&self, _sender: Option<&AnyObject>) {
                send_volume(None, Some(-0.1), None);
            }

            #[unsafe(method(mute:))]
            fn mute(&self, _sender: Option<&AnyObject>) {
                send_volume(None, None, Some(true));
            }

            #[unsafe(method(unmute:))]
            fn unmute(&self, _sender: Option<&AnyObject>) {
                send_volume(None, None, Some(false));
            }

            #[unsafe(method(quit:))]
            fn quit(&self, _sender: Option<&AnyObject>) {
                let mtm = MainThreadMarker::from(self);
                NSApplication::sharedApplication(mtm).terminate(None);
            }
        }
    );

    impl Tray {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm);
            unsafe { msg_send![this, init] }
        }
    }

    fn item(
        mtm: MainThreadMarker,
        title: &NSString,
        action: objc2::runtime::Sel,
        target: &Tray,
    ) -> Retained<NSMenuItem> {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                title,
                Some(action),
                ns_string!(""),
            )
        };
        unsafe { item.setTarget(Some(target)) };
        item
    }

    pub fn run() -> Result<()> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| anyhow::anyhow!("tray must run on the main thread"))?;
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let target = Tray::new(mtm);
        let menu = NSMenu::new(mtm);
        menu.addItem(&item(
            mtm,
            ns_string!("Volume Up"),
            sel!(volumeUp:),
            &target,
        ));
        menu.addItem(&item(
            mtm,
            ns_string!("Volume Down"),
            sel!(volumeDown:),
            &target,
        ));
        menu.addItem(&item(mtm, ns_string!("Mute"), sel!(mute:), &target));
        menu.addItem(&item(mtm, ns_string!("Unmute"), sel!(unmute:), &target));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&item(mtm, ns_string!("Quit Tray"), sel!(quit:), &target));

        let status_bar = NSStatusBar::systemStatusBar();
        let status_item: Retained<NSStatusItem> =
            status_bar.statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = status_item.button(mtm) {
            button.setTitle(ns_string!("\u{266A}"));
        }
        status_item.setMenu(Some(&menu));

        app.run();
        Ok(())
    }
}
