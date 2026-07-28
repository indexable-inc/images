//! Native right-click context menu for an overlay window.
//!
//! winit exposes no menu API, so on macOS we drop to AppKit: build an `NSMenu`,
//! attach a tiny `NSObject` target that records which item the user picked, and
//! pop it up at the pointer. `-[NSMenu popUpMenuPositioningItem:atLocation:inView:]`
//! runs its own modal tracking loop and returns once the user selects an item or
//! dismisses the menu, so [`popup`] blocks and the chosen index is known by the
//! time it returns.
//!
//! Off macOS there is no single portable native menu, so [`popup`] is a no-op that
//! returns `None` (a right-click simply does nothing).

/// Pop up a native context menu at the current pointer location with one entry per
/// `items` label, blocking until the user picks one or dismisses it. Returns the
/// index of the chosen label, or `None` if dismissed (or off macOS).
#[cfg(target_os = "macos")]
pub fn popup(items: &[&str]) -> Option<usize> {
    use std::cell::Cell;

    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{declare_class, msg_send_id, mutability, sel, ClassType, DeclaredClass};
    use objc2_app_kit::{NSEvent, NSMenu, NSMenuItem};
    use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSString};

    /// Records the tag of the menu item the user selected; -1 until then.
    struct Ivars {
        selected: Cell<isize>,
    }

    declare_class!(
        struct Target;

        unsafe impl ClassType for Target {
            type Super = NSObject;
            type Mutability = mutability::MainThreadOnly;
            const NAME: &'static str = "OverlayMenuTarget";
        }

        impl DeclaredClass for Target {
            type Ivars = Ivars;
        }

        unsafe impl NSObjectProtocol for Target {}

        unsafe impl Target {
            // The action every item is wired to: stash the sender's tag (its index)
            // so the caller learns which entry was picked once tracking ends.
            #[method(overlayMenuAction:)]
            fn action(&self, sender: Option<&NSMenuItem>) {
                if let Some(item) = sender {
                    self.ivars().selected.set(unsafe { item.tag() });
                }
            }
        }
    );

    impl Target {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = mtm.alloc::<Self>().set_ivars(Ivars {
                selected: Cell::new(-1),
            });
            unsafe { msg_send_id![super(this), init] }
        }
    }

    // Menus are main-thread only; we are called from the winit event loop, which
    // is the main thread. If somehow not, decline rather than risk UB.
    let mtm = MainThreadMarker::new()?;
    if items.is_empty() {
        return None;
    }

    let target = Target::new(mtm);
    let menu = NSMenu::new(mtm);
    let action = sel!(overlayMenuAction:);
    let empty = NSString::new();
    for (i, label) in items.iter().enumerate() {
        let title = NSString::from_str(label);
        // Items default to auto-enabled because `target` responds to the action.
        let item =
            unsafe { menu.addItemWithTitle_action_keyEquivalent(&title, Some(action), &empty) };
        unsafe {
            // `setTarget:` holds a weak reference, so `target` must outlive the
            // pop-up below; it does (it lives to the end of this function).
            item.setTarget(Some(&target));
            item.setTag(i as isize);
        }
    }

    // With no view, the location is the screen coordinate system, which is exactly
    // what `mouseLocation` returns, so the menu opens under the pointer.
    let loc = unsafe { NSEvent::mouseLocation() };
    unsafe { menu.popUpMenuPositioningItem_atLocation_inView(None, loc, None) };

    let selected = target.ivars().selected.get();
    (selected >= 0).then_some(selected as usize)
}

#[cfg(not(target_os = "macos"))]
pub fn popup(_items: &[&str]) -> Option<usize> {
    None
}
