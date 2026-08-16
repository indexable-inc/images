# `readFileType` of a path that is not there.
#
# This pins the throw that ENG-13123 MOVED. Before that change the bridge
# asked cppnix's `SourceAccessor::lstat`, which is `maybeLstat` plus
# `throw FileNotFound`, and handed the Rust VM the finished message. After it,
# the bridge answers the datum `absent` and the VM raises the error itself,
# because the ancestor scan needed absence to be a value rather than a
# failure and both callers read through the one accessor method.
#
# Moving a throw across a boundary is exactly the kind of change that keeps
# working for the caller you were thinking about and stops working for the
# one you were not, and nothing in this corpus was watching this caller:
# there are cases here for `readFileType` of a file, of a symlink and of a
# symlinked ancestor, and there was none for a path that simply does not
# exist. So the behaviour this fix promises to preserve was unpinned while
# the behaviour it changes got three new cases.
#
# What this case does NOT cover, stated so the next person does not assume
# it does: the message is built from the accessor's `showPath`, which is
# `displayPrefix + path + displaySuffix`, and here the prefix is empty. An
# accessor that sets one -- a relocated store, or `RemoteFSAccessor`, whose
# prefix stays at the base class's `«unknown»` -- would show a different
# string, and the corpus cannot reach either of those.
builtins.readFileType ./eng13123-no-such-file
