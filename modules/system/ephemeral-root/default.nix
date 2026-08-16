# The root filesystem is thrown away on every boot, and only a named
# whitelist survives.
#
# WHAT THIS BUYS. A machine's configuration is in the flake, but its *state*
# accumulates outside it: a package installed by hand at 2am, a stale
# `/etc/foo.conf` a service wrote once, a settings file an application decided
# to rewrite. None of that is in the generation, so a deploy does not remove
# it and no check can see it. Two machines built from the same revision drift
# apart and the difference stays invisible until something breaks on one of
# them. Wiping the root at boot makes the generation the only thing that
# survives, so "it works after a reboot" and "it is in the config" become the
# same statement.
#
# It also enforces managed configuration. A file placed by home-manager as a
# read-only store symlink can be replaced by an application that unlinks and
# rewrites it; the symlink returns on the next activation, but whatever the
# application wrote lives until then. Here it lives until the next boot at the
# outside, because a path absent from the whitelist has no way to reach the
# next generation.
#
# THE SHAPE. Four pieces, one module, because they are only correct together:
#
#   1. The whitelist. Bind mounts (and symlinks) from a persistent filesystem
#      onto the wiped root. This is the entire user-facing surface.
#   2. The seed. Whitelisted sources are created before anything mounts them,
#      with declared ownership, because a bind mount whose source does not
#      exist fails the boot.
#   3. The rollback. An initrd unit that replaces the root with a blank
#      snapshot before the root is mounted.
#   4. The audit. An eval-time cross-check of every service's `StateDirectory`
#      against the whitelist, so a service whose state is about to be wiped
#      fails the build instead of failing at 3am.
#
# WHY ONE MODULE AND NOT FOUR. A whitelist with no wipe is harmless. A wipe
# with no whitelist is a machine that loses its identity every boot: a fresh
# `/etc/machine-id` invalidates the journal and every service keyed on it, and
# an empty `/var/lib/nixos` re-rolls uid/gid allocations so every file on the
# persistent filesystem ends up owned by the wrong user. Keeping them together
# lets the system seed below be unconditional rather than an option somebody
# can forget, which is the difference between a class of failure that is
# documented and one that cannot be expressed.
#
# WHY NOT `preservation` OR `impermanence`. Both are third-party flake inputs,
# and a module in this directory receives none: `index/lib/discovery.nix`
# exposes each `default.nix` as a bare path, evaluated with the *consumer's*
# module arguments, so an input added here would have to be added to every
# consumer's flake as well. The mechanism they implement is the `fileSystems`
# bind entries and tmpfiles rules generated below; taking the dependency would
# buy option names, not behaviour. The cost is real and worth stating: their
# option surface is richer, and piece 4 above is the only thing here that
# neither of them has.
#
# WHAT THIS DOES NOT DO. There is no live wipe. The wipe point is boot and only
# boot, so "reset this machine" means "reboot it". A live wipe would have to
# delete a running system's root and then re-create every bind mount and every
# home-manager symlink underneath the processes still holding them open, which
# means tracking the whitelist in two places forever. A reboot runs where
# nothing holds a file open and costs about thirty seconds.
{
  config,
  ix,
  lib,
  pkgs,
  utils,
  ...
}: let
  inherit
    (lib)
    concatMapStringsSep
    concatStringsSep
    hasPrefix
    mkDefault
    mkEnableOption
    mkIf
    mkOption
    optionalAttrs
    types
    ;

  cfg = config.system.ephemeralRoot;

  entryModule = {config, ...}: {
    options = {
      path = mkOption {
        type = types.str;
        example = "/var/lib/tailscale";
        description = ''
          Absolute path on the wiped root that survives the next boot. Its
          source is the same path under `persistentRoot`.
        '';
      };

      how = mkOption {
        type = types.enum ["bind" "symlink"];
        default = "bind";
        description = ''
          `bind` mounts the persistent copy over the path. `symlink` places a
          symlink pointing at it.

          Prefer `bind`. It is invisible to whatever reads the path, and it
          keeps the path a mount boundary, which is what lets
          `ix-wipe-preview` list what dies by walking what is not one.

          Use `symlink` for a path whose writer replaces it by renaming a
          temporary file over it. That rename returns EBUSY against a bind
          mount and succeeds through a symlink. SSH host keys are the known
          case; an application that saves settings atomically is another.
        '';
      };

      inInitrd = mkOption {
        type = types.bool;
        default = false;
        description = ''
          Whether the path is needed before stage 2 starts. Sets
          `neededForBoot` on the generated mount and moves the seed into the
          initrd.

          An `inInitrd` entry must be root-owned, and an assertion enforces
          that: the initrd seed runs `systemd-tmpfiles --root=/sysroot`
          against a root that was just blanked, so there is no `/etc/passwd`
          under there to resolve a name against.

          Required for anything under `/etc`, because NixOS activation runs in
          the initrd and writes `/etc` there; a second assertion enforces that.
          The entries that need it (`/etc/machine-id`, `/var/lib/nixos`,
          `/etc/ssh`) are set by this module already and are the usual complete
          list.
        '';
      };

      isFile = mkOption {
        type = types.bool;
        default = false;
        description = ''
          Whether the path is a single file rather than a directory. Decides
          whether the seed creates a file or a directory on the persistent
          side, which a bind mount requires to match.
        '';
      };

      user = mkOption {
        type = types.str;
        default = "root";
        description = "Owner of the persistent source, applied when the seed creates it.";
      };

      group = mkOption {
        type = types.str;
        default = "root";
        description = "Group of the persistent source, applied when the seed creates it.";
      };

      mode = mkOption {
        type = types.str;
        default = "0755";
        description = ''
          Mode of the persistent source, applied when the seed creates it.
          Defaults to `0755`; an `isFile` entry defaults to `0644` instead
          (set below at default priority, so any explicit value wins).
        '';
      };
    };

    config.mode = mkIf config.isFile (mkDefault "0644");
  };

  entryType = types.submodule entryModule;

  # Paths that must survive or the machine does not come back as itself. These
  # are merged into `entries` rather than offered as options: see the module
  # comment on why the wipe and the seed cannot be separated.
  systemSeed = [
    # The uid/gid allocations nixpkgs made for every declarative user without
    # a fixed id. Lose this and the next boot hands out different numbers, at
    # which point every file on the persistent filesystem is owned by a
    # stranger. This is the most expensive thing here to get wrong, because it
    # is silent: the machine boots, and the damage surfaces later as
    # permission errors on data that was fine yesterday.
    {
      path = "/var/lib/nixos";
      inInitrd = true;
    }
    {
      path = "/etc/machine-id";
      inInitrd = true;
      isFile = true;
    }
    # Symlinked, not bound: sshd and ssh-keygen rewrite a host key by writing
    # a temporary file and renaming it over the target, and that rename
    # returns EBUSY against a bind-mounted file.
    #
    # `inInitrd` because activation runs there and writes /etc. Set up after
    # it, this symlink would be `L+` over the real `/etc/ssh` that
    # `setup-etc.pl` had just filled with `ssh_config` and `sshd_config`, and
    # `L+` removes a directory to make room for the link. Set up before it,
    # activation follows the link and writes those files into the persistent
    # copy, which is where they should be. The assertion below holds this for
    # every /etc entry.
    {
      path = "/etc/ssh";
      how = "symlink";
      inInitrd = true;
    }
    # Without this the machine has no history across the only event that ever
    # wipes it, which is exactly when history is wanted.
    {path = "/var/log";}
    # systemd writes the last-run time of a `Persistent=true` timer here.
    # Losing it makes every such timer fire on every boot.
    {path = "/var/lib/systemd/timers";}
  ];

  # One submodule evaluation for every entry from every source, so a
  # hand-written `systemSeed` record above and a user-supplied `entries` record
  # come out with the same defaults filled in. Without this the seed records
  # would be missing `how`, `user`, `mode` and friends at every use below.
  applyDefaults = entries:
    map (entry: (lib.evalModules {modules = [entryModule {config = entry;}];}).config) entries;

  # A user entry is an ordinary entry with the home directory prepended and
  # ownership defaulted to that user, so `.ssh` does not have to be spelled
  # `/home/andrew/.ssh` with `user = "andrew"` at every call site.
  #
  # WHY `home` AND `group` ARE RESTATED HERE RATHER THAN READ FROM
  # `users.users.<name>`, WHICH IS WHERE THEY ALREADY LIVE. Reading them makes
  # `fileSystems` depend on `users.users`, and nixpkgs derives
  # `boot.supportedFilesystems` from `fileSystems`
  # (nixos/modules/tasks/filesystems.nix:52) while `nfs.nix` turns that back
  # into `services.rpcbind.enable`, which defines a user. That closes a cycle
  # and the eval dies with `infinite recursion` naming none of the four
  # modules involved. Measured: the identical config evaluates once the user
  # entries are dropped.
  #
  # So the two facts are written twice, and the assertion below is what keeps
  # them equal. A restated fact with a check on it is worse than a derived one
  # and much better than a silent divergence; the alternative was dropping
  # per-user whitelists entirely.
  userEntries =
    lib.concatLists
    (lib.mapAttrsToList
      (userName: userCfg:
        map (entry:
          entry
          // {
            path = "${userCfg.home}/${entry.path}";
            user =
              if entry.user == "root"
              then userName
              else entry.user;
            group =
              if entry.group == "root"
              then userCfg.group
              else entry.group;
          })
        userCfg.entries)
      cfg.users);

  # Every per-user field that is restated above, paired with the value it is
  # supposed to equal. Read here rather than in `userEntries` because an
  # assertion is not in the `fileSystems` dependency path, so this closes the
  # divergence without reopening the cycle.
  userMismatches =
    lib.concatLists
    (lib.mapAttrsToList (userName: userCfg: let
      declared = config.users.users.${userName} or null;
    in
      lib.optionals (declared != null) (
        lib.optional (declared.home != userCfg.home)
        "  users.${userName}.home = \"${userCfg.home}\" but users.users.${userName}.home = \"${declared.home}\""
        ++ lib.optional (declared.group != userCfg.group)
        "  users.${userName}.group = \"${userCfg.group}\" but users.users.${userName}.group = \"${declared.group}\""
      ))
    cfg.users);

  allEntries = applyDefaults systemSeed ++ cfg.entries ++ userEntries;

  initrdEntries = builtins.filter (e: e.inInitrd) allEntries;
  etcEntries = builtins.filter (e: lib.hasPrefix "/etc/" e.path || e.path == "/etc") allEntries;
  bindEntries = builtins.filter (e: e.how == "bind") allEntries;
  symlinkEntries = builtins.filter (e: e.how == "symlink") allEntries;

  sourceOf = entry: "${cfg.persistentRoot}${entry.path}";

  # THE SEED, as systemd-tmpfiles rules rather than a shell script.
  #
  # One generator, so the thing that creates a source and the thing that
  # mounts it cannot disagree about its type, mode or owner. tmpfiles is also
  # idempotent by construction, which a hand-written `mkdir -p; chmod; chown`
  # sequence only is if nobody ever edits it.
  #
  # Rendered to files rather than declared through `systemd.tmpfiles.settings`
  # because the initrd cannot reach that: NixOS renders those into
  # `/etc/tmpfiles.d` on the root filesystem, and at the moment the initrd
  # needs them that filesystem has just been blanked.
  tmpfilesLine = target: entry:
    concatStringsSep " " [
      (
        if entry.isFile
        then "f"
        else "d"
      )
      target
      entry.mode
      entry.user
      entry.group
      "-"
    ];

  # The `L+` lines are filtered to this file's own entries and not taken from
  # `symlinkEntries`, which is every symlink entry in the config. The initrd
  # file used to carry all of them: it created `/etc/ssh` pointing at a
  # `/persistent/etc/ssh` whose `d` line lived in the other file, so the
  # symlink dangled until stage 2. Activation runs inside the initrd, before
  # that, and `setup-etc.pl` calls `make_path` on the dangling symlink, where
  # `-d` is false and `mkdir` then fails EEXIST. `/etc` was left half-written,
  # stage 2 found no `default.target`, and systemd froze (measured 2026-08-06).
  # A rules file that names a source it does not create is the general form of
  # that, so the fix is here rather than on the one entry.
  mkRulesFile = name: entries:
    pkgs.writeText name (concatStringsSep "\n"
      (map (entry: tmpfilesLine (sourceOf entry) entry) entries
        # `L+` replaces whatever is at the path, which on a freshly blanked
        # root is nothing, and on a machine still growing its whitelist is
        # whatever was there before the entry was added.
        ++ map (entry: "L+ ${entry.path} - - - - ${sourceOf entry}")
        (builtins.filter (e: e.how == "symlink") entries))
      + "\n");

  allRules = mkRulesFile "ephemeral-root.conf" allEntries;
  initrdRules = mkRulesFile "ephemeral-root-initrd.conf" initrdEntries;

  # THE AUDIT. `StateDirectory=foo` means the service writes to /var/lib/foo
  # and expects to find it again next boot. That is a fact about the
  # generation, readable at eval time, so a service whose state is about to be
  # wiped is a build error rather than a discovery.
  #
  # This is the piece that turns "grow the whitelist from evidence" into
  # "the build tells you first". Evidence still grows it, but only for state
  # that no `StateDirectory=` declares.
  stateDirectoriesOf = service:
    lib.toList (service.serviceConfig.StateDirectory or []);

  declaredStateDirectories =
    lib.unique
    (lib.concatMap stateDirectoriesOf (builtins.attrValues config.systemd.services));

  isCovered = path:
    lib.any
    (entry: entry.path == path || hasPrefix "${entry.path}/" path)
    allEntries;

  unwhitelistedStateDirectories =
    builtins.filter
    (dir: !(builtins.elem dir cfg.ephemeralStateDirectories) && !(isCovered "/var/lib/${dir}"))
    declaredStateDirectories;

  # A typed Python tool rather than generated shell, which the shell fence
  # froze (#3823). Everything the next boot destroys: the walk stays on the
  # root filesystem's device (`find -xdev` semantics; see the script's
  # docstring for why that is the whole mechanism), and the symlinked
  # whitelist entries ride in as argv so the eval-time facts stay out of the
  # compiled-once script.
  wipePreview = ix.writePythonApplication pkgs {
    name = "ix-wipe-preview";
    src = ./wipe-preview.py;
    args = map (e: e.path) symlinkEntries;
    meta.description = "Preview what modules/system/ephemeral-root wipes on the next boot";
  };

  # RETENTION IS A COUNT, AND THE SAME COUNT FOR ALL THREE METHODS. Written
  # once here because each script spells its own "keep the newest N" in its own
  # tooling, and two of them are one-off pipelines where an off-by-one is
  # invisible until the day something has to be recovered.
  #
  # `tail -n +K` starts at line K, so the first line to delete is the one after
  # the last line to keep.
  firstDoomedLine = toString (cfg.rollback.keepGenerations + 1);

  # `vg/lv` is how every lvm2 tool names a volume, and `volumeGroup` is null
  # until the method is `lvmthin`, where an assertion requires it. `toString`
  # on a null yields the empty string rather than an eval error, so this stays
  # buildable for the other methods, where the string is never used.
  vgPath = volume: "${toString cfg.rollback.volumeGroup}/${volume}";

  rollbackScripts = {
    # Move the old root aside, then prune to the newest `keepGenerations`. The
    # undo button: a boot that took something unwhitelisted with it leaves the
    # evidence under /old_roots, which is the difference between "I lost it"
    # and "I know where it went".
    btrfs = ''
      mkdir -p /ephemeral-root-top
      mount -t btrfs -o subvolid=5 "${toString cfg.rollback.device}" /ephemeral-root-top

      # Deleting a subvolume does not delete the subvolumes nested inside it,
      # so a plain `btrfs subvolume delete` on an old root leaves those behind
      # and the prune quietly stops reclaiming anything. `-o` lists what is
      # below the argument; field 9 onward is the path relative to the top.
      delete_subvolume_recursively() {
        local child
        while read -r child; do
          delete_subvolume_recursively "/ephemeral-root-top/$child"
        done < <(btrfs subvolume list -o "$1" | cut -f 9- -d ' ')
        btrfs subvolume delete "$1"
      }

      if [ -e "/ephemeral-root-top/${cfg.rollback.subvolume}" ]; then
        mkdir -p /ephemeral-root-top/old_roots
        stamp=$(date --date="@$(stat -c %Y "/ephemeral-root-top/${cfg.rollback.subvolume}")" "+%Y-%m-%dT%H:%M:%S")
        mv "/ephemeral-root-top/${cfg.rollback.subvolume}" "/ephemeral-root-top/old_roots/$stamp"
      fi

      if [ -d /ephemeral-root-top/old_roots ]; then
        # Newest first, then everything past the keep count. Sorting the whole
        # path works because they share a prefix and the stamp is written
        # biggest unit first, so lexicographic order is time order. `find`
        # without `-printf`, which busybox does not implement.
        find /ephemeral-root-top/old_roots/ -mindepth 1 -maxdepth 1 \
          | sort -r \
          | tail -n "+${firstDoomedLine}" \
          | while read -r old; do
            delete_subvolume_recursively "$old"
          done
      fi

      btrfs subvolume snapshot \
        "/ephemeral-root-top/${cfg.rollback.blankSnapshot}" \
        "/ephemeral-root-top/${cfg.rollback.subvolume}"

      umount /ephemeral-root-top
    '';

    # ZFS keeps its own undo, but only if one is made: `zfs rollback -r`
    # destroys every snapshot newer than the target without asking. Taking one
    # first is the same decision as moving the btrfs subvolume aside.
    zfs = ''
      stamp=$(date "+%Y-%m-%dT%H:%M:%S")
      zfs snapshot "${toString cfg.rollback.dataset}@old-root-$stamp"
      zfs rollback -r "${toString cfg.rollback.dataset}@${cfg.rollback.blankSnapshot}"

      # Oldest first (`-s creation`), then all but the last `keepGenerations`.
      # `awk` and not `grep`: grep exits 1 when it matches nothing, and under
      # `set -o pipefail` that turns the first boot, where there is nothing to
      # prune yet, into a failed boot.
      zfs list -H -p -t snapshot -o name -s creation -r "${toString cfg.rollback.dataset}" \
        | awk '/@old-root-/' \
        | head -n "-${toString cfg.rollback.keepGenerations}" \
        | while read -r old; do
          zfs destroy "$old"
        done
    '';

    # THE THIN-SNAPSHOT METHOD. Structurally the btrfs one: rename the current
    # root out of the way for retention, then make a fresh snapshot of the
    # blank volume under the name the root mounts from.
    #
    # VERIFIED ON A RUNNING MACHINE by tests/ephemeral-root-lvmthin-vm.nix,
    # which installs a real GPT + LVM thin pool with disko and boots the
    # installed system off it twice. Before that test existed every claim below
    # was argued from lvcreate(8) and lvm(8) and none was measured; the first
    # boot found a missing `awk` that killed the prune (see `initrdBin` below).
    #
    # THE PART THAT WAS WORTH WATCHING, and now has been. Renaming and
    # re-creating `rootVolume` destroys and re-creates its device node, and
    # `sysroot.mount` carries a `Requires=` on the `.device` unit for whatever
    # `fileSystems."/"` names. This unit is ordered before that mount, so the
    # node should be back before anything needs it, but a device unit that goes
    # away after systemd has already seen it was a state nothing had produced.
    # The test brackets this unit with two probes that record
    # `dev-<vg>-<lv>.device` from systemd's own point of view and asserts the
    # unit was active before, is backed by a different dm device after, and is
    # active again. The ordering holds. If a machine ever does end up in the
    # emergency shell with the root LV present and active, that is still where
    # to look first.
    lvmthin = ''
      # The stamp uses `-` and not `:` between the time fields: an LV name is
      # limited to [a-zA-Z0-9+_.-], and lvrename rejects a colon outright, so
      # the btrfs spelling would fail the boot here.
      stamp=$(date "+%Y-%m-%dT%H-%M-%S")

      # Absent on the very first boot after an install that made only the blank
      # volume, which is not an error: there is no old root to keep.
      if lvs "${vgPath cfg.rollback.rootVolume}" > /dev/null 2>&1; then
        lvrename "${toString cfg.rollback.volumeGroup}" \
          "${cfg.rollback.rootVolume}" "old-root-$stamp"
      fi

      # `--setactivationskip n` because lvcreate(8) sets the activation-skip
      # flag on a thin snapshot by default, and an LV that is skipped at
      # activation has no device node for `sysroot.mount` to find, which is a
      # boot that ends in the emergency shell rather than a boot that keeps
      # too much. The explicit `lvchange` covers this boot; the cleared flag
      # covers every later one.
      #
      # Neither line is redundant, and the order matters, which was measured
      # rather than reasoned: with `--setactivationskip n` removed and the
      # `lvchange` left in, the boot still died in the emergency shell
      # ("systemd-fsck-root.service: Bound to unit dev-vg0-root.device, but
      # unit isn't active", then "Reached target Emergency Mode"). A plain
      # `lvchange -ay` honours the skip flag too; overriding it would need
      # `-K`. So the flag has to be cleared at creation for the activation
      # below to do anything. Negative control (b) in
      # tests/ephemeral-root-lvmthin-vm.nix.
      lvcreate --snapshot --setactivationskip n \
        --name "${cfg.rollback.rootVolume}" "${vgPath cfg.rollback.blankVolume}"
      lvchange --activate y "${vgPath cfg.rollback.rootVolume}"

      # Newest first, then everything past the keep count. `awk` and not
      # `grep` for the same reason as the zfs case above.
      lvs --noheadings -o lv_name "${toString cfg.rollback.volumeGroup}" \
        | tr -d ' ' \
        | awk '/^old-root-/' \
        | sort -r \
        | tail -n "+${firstDoomedLine}" \
        | while read -r old; do
          lvremove --yes "${toString cfg.rollback.volumeGroup}/$old"
        done
    '';
  };

  # The mount unit name systemd derives for a path inside the initrd, where the
  # new root is at /sysroot. Computed rather than written out so each path
  # stays one string.
  #
  # `utils.escapeSystemdPath` and not a hand-rolled `/` to `-` substitution:
  # systemd spells a literal dash `\x2d`, so a path like `/var-persist` would
  # yield a unit name nothing instantiates. Named in a `Requires=` that fails
  # loudly, but the same mistake in the rollback's `After=` was silent for a
  # day (see that unit's comment).
  #
  # The helper mis-escapes `.` segments and exotic characters
  # (nixpkgs#515270), so `escapedPaths` below is asserted to the shape it does
  # handle, matching packages/panes/guest-image/nixos.nix.
  initrdMountUnitFor = path: "${utils.escapeSystemdPath "/sysroot${path}"}.mount";

  persistentInitrdMountUnit = initrdMountUnitFor cfg.persistentRoot;

  # The initrd seed writes the source of each of these, so each mount has to
  # wait for it. `Before=initrd-fs.target` on the seed does not arrange that:
  # the mounts are before the same target, which leaves them free to run
  # alongside. One did, created `/persistent/etc/machine-id` as a directory
  # where a file belonged, and left the machine unable to read its own
  # machine-id (2026-08-06). The two runs before it had passed on timing alone.
  initrdBindEntries = builtins.filter (entry: entry.how == "bind") initrdEntries;

  initrdBindMountUnits = map (entry: initrdMountUnitFor entry.path) initrdBindEntries;

  # Every path a mount unit name is derived from, so one assertion covers them.
  escapedPaths = [cfg.persistentRoot] ++ map (entry: entry.path) initrdBindEntries;
in {
  options.system.ephemeralRoot = {
    enable = mkEnableOption ''
      wiping the root filesystem on every boot, keeping only whitelisted paths.

      Enabling this alone does not wipe anything: `rollback.method` still has
      to name a mechanism, and that mechanism needs a blank snapshot only the
      installer can make. Enabling it by itself gives the whitelist, the
      audit and the preview tool with no wipe, which is the intended way to
      grow a whitelist on a live machine before committing to the reinstall
    '';

    persistentRoot = mkOption {
      type = types.str;
      default = "/persistent";
      description = ''
        Filesystem that survives the wipe. Every whitelisted path is stored
        beneath it at the same path it has on the root.

        It must be a real mount that the rollback does not touch, and it must
        be `neededForBoot`, because the initrd seeds `/etc/machine-id` and
        `/var/lib/nixos` into it before stage 2 exists.
      '';
    };

    entries = mkOption {
      type = types.listOf entryType;
      default = [];
      example = lib.literalExpression ''
        [
          {path = "/var/lib/tailscale";}
          {
            path = "/var/lib/postgresql";
            user = "postgres";
            group = "postgres";
          }
        ]
      '';
      description = ''
        System paths that survive the wipe, on top of the ones this module
        always keeps (`/var/lib/nixos`, `/etc/machine-id`, `/etc/ssh`,
        `/var/log`, `/var/lib/systemd/timers`).

        Add an entry in the same commit as the thing that needs it. For state
        a service declares through `StateDirectory=`, the build will tell you:
        see `ephemeralStateDirectories`.
      '';
    };

    users = mkOption {
      default = {};
      description = ''
        Per-user whitelists. Paths are relative to the user's home directory
        and default to being owned by that user.
      '';
      type = types.attrsOf (types.submodule ({name, ...}: {
        options = {
          entries = mkOption {
            type = types.listOf entryType;
            default = [];
            example = lib.literalExpression ''
              [
                {path = ".ssh";}
                {path = ".local/share/fish";}
              ]
            '';
            description = "Paths under this user's home that survive the wipe.";
          };

          home = mkOption {
            type = types.str;
            default = "/home/${name}";
            description = ''
              This user's home directory, which every entry above is relative
              to. It must equal `users.users.${name}.home`, and an assertion
              checks that it does.

              It is stated again here rather than read from there because
              reading it puts `fileSystems` downstream of `users.users` and
              the evaluation becomes an infinite recursion through
              `boot.supportedFilesystems`. The reasoning is in the comment on
              `userEntries`.
            '';
          };

          group = mkOption {
            type = types.str;
            default = "users";
            description = ''
              This user's primary group, used as the default group of entries
              above. Must equal `users.users.${name}.group`; same assertion,
              same reason.
            '';
          };
        };
      }));
    };

    ephemeralStateDirectories = mkOption {
      type = types.listOf types.str;
      default = [];
      example = ["nginx" "fail2ban"];
      description = ''
        `StateDirectory=` names whose state is deliberately allowed to die on
        every boot.

        Every service that declares `StateDirectory=foo` is asserting that
        `/var/lib/foo` survives a reboot. Under this module most of them are
        wrong unless somebody looked. So the build fails on any such
        directory that is neither whitelisted nor named here, and the choice
        is recorded either way. A new service that ships with a
        `StateDirectory=` cannot quietly start losing its state.
      '';
    };

    bindMounts = mkOption {
      type = types.attrsOf types.anything;
      internal = true;
      readOnly = true;
      description = ''
        The `fileSystems` entries this module contributes, exposed so that a
        consumer which assembles `fileSystems` itself can merge them in.

        `fileSystems` below is defined as exactly this value, so there is one
        computation and no second copy to drift. The consumer that needs it is
        the NixOS VM test harness: `qemu-vm.nix` sets
        `fileSystems = mkVMOverride cfg.fileSystems`, priority 10, which
        replaces the whole attribute set rather than merging into it. Every
        bind this module declares is dropped inside a VM, silently, and the
        machine boots with a whitelist that mounts nothing.
      '';
    };

    rollback = {
      method = mkOption {
        type = types.enum ["none" "btrfs" "zfs" "lvmthin"];
        default = "none";
        description = ''
          How the root is returned to blank.

          `none` installs the whitelist, the seed, the audit and the preview
          tool, and performs no wipe. That is the staging state, and it is
          the default so that enabling this module can never destroy a
          filesystem nobody prepared.

          `btrfs` replaces `rollback.subvolume` with a fresh snapshot of
          `rollback.blankSnapshot`, moving the old one under `/old_roots`.

          `zfs` rolls `rollback.dataset` back to `rollback.blankSnapshot`,
          snapshotting the current state first.

          `lvmthin` replaces `rollback.rootVolume` with a fresh thin snapshot
          of `rollback.blankVolume`, renaming the old one aside. It is the
          method for a machine whose root is an ordinary filesystem on a thin
          LV, so the root keeps whatever filesystem it already had. It is also
          the only one of the three not exercised by a VM test in this repo:
          see the comment on its script.
        '';
      };

      device = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "/dev/disk/by-partlabel/disk-system-root";
        description = ''
          btrfs only. The block device holding the top-level (subvolid 5)
          btrfs filesystem.

          Name it by a stable path. This string becomes a dependency of an
          initrd unit that mounts it before anything else has, so a kernel
          name that moved between boots takes the machine to an emergency
          shell.
        '';
      };

      subvolume = mkOption {
        type = types.str;
        default = "root";
        description = ''
          btrfs only. The subvolume mounted at `/`, named relative to the
          btrfs top level.

          It must not BE the top level: subvolid 5 cannot be replaced, so a
          layout that mounts it directly as the root cannot be rolled back at
          all. The layout that produces this is the authority on the name;
          keep the two in one place rather than spelling it here and there.
        '';
      };

      dataset = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "rpool/root";
        description = "zfs only. The dataset mounted at `/`.";
      };

      blankSnapshot = mkOption {
        type = types.str;
        default = "root-blank";
        description = ''
          The empty snapshot every boot returns to, taken at install time
          against a root that has never been booted.

          For `btrfs` this is a path relative to the top level; for `zfs` a
          snapshot name on `dataset`. `lvmthin` has no snapshot namespace of
          its own and uses `blankVolume` instead.
        '';
      };

      volumeGroup = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "vg0";
        description = ''
          lvmthin only. The volume group holding `blankVolume` and
          `rootVolume`, both of which must be thin LVs in the same pool: a
          snapshot cannot cross pools, and `lvcreate --snapshot` without a
          size is only valid against a thin volume.
        '';
      };

      blankVolume = mkOption {
        type = types.str;
        default = "root-blank";
        description = ''
          lvmthin only. The empty thin LV every boot snapshots from, made and
          mkfs'd once at install time.

          It must never be mounted read-write. It is the origin of every root
          this machine will ever boot, so anything written to it is written to
          all of them, and there is no earlier copy to go back to.
        '';
      };

      rootVolume = mkOption {
        type = types.str;
        default = "root";
        description = ''
          lvmthin only. The thin LV mounted at `/`, replaced on every boot by
          a fresh snapshot of `blankVolume`.

          `fileSystems."/"` must name this same volume. Nothing checks that
          they agree, because the device there may be spelled by any of
          several stable paths.
        '';
      };

      keepGenerations = mkOption {
        type = types.ints.positive;
        default = 3;
        description = ''
          How many displaced old roots are kept before the oldest is deleted.
          The root this boot just displaced is the first of them, so the
          default keeps the previous boot's root and the two before it.

          This is the window in which a path that should have been
          whitelisted can still be recovered.

          A count and not a number of days because of what runs out under
          `lvmthin`: every retained thin snapshot holds a share of the pool's
          metadata volume, and a metadata volume that fills takes the entire
          pool read-only until an offline `thin_repair`. That is a fleet
          outage recovered by hand, and a count is a hard bound on the number
          of snapshots that can cause it. A day count bounds nothing, because
          the number of roots a day produces is the number of times the
          machine rebooted that day. btrfs and zfs are held to the same count
          so there is one retention concept to reason about rather than two.
        '';
      };
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        # The scripted initrd ignores `boot.initrd.systemd.services.*` rather
        # than rejecting it, so without this the module's whole initrd half
        # disappears: the machine boots normally, keeps everything, and looks
        # exactly like a machine where the wipe is working. That is the one
        # failure this module cannot afford to have go quiet, so it is an
        # assertion rather than a warning.
        assertion = config.boot.initrd.systemd.enable;
        message = ''
          system.ephemeralRoot requires boot.initrd.systemd.enable = true. The scripted
          initrd silently drops the rollback unit, so the root is never wiped and nothing
          reports it.
        '';
      }
      {
        assertion = cfg.rollback.method != "btrfs" || cfg.rollback.device != null;
        message = "system.ephemeralRoot.rollback.method = \"btrfs\" requires rollback.device.";
      }
      {
        assertion = cfg.rollback.method != "zfs" || cfg.rollback.dataset != null;
        message = "system.ephemeralRoot.rollback.method = \"zfs\" requires rollback.dataset.";
      }
      {
        assertion = cfg.rollback.method != "lvmthin" || cfg.rollback.volumeGroup != null;
        message = "system.ephemeralRoot.rollback.method = \"lvmthin\" requires rollback.volumeGroup.";
      }
      {
        # `services.lvm.boot.thin.enable` below only reaches the initrd through
        # this option (nixpkgs nixos/modules/tasks/lvm.nix:90), and the udev
        # rules that activate the volume group come from the same place. With
        # it off the machine has no volume group in the initrd at all, so the
        # root LV never appears and the boot ends in the emergency shell. It
        # defaults to on wherever `boot.initrd.systemd.enable` is, which this
        # module already requires, so this fires only for a config that turned
        # LVM off by hand.
        assertion = cfg.rollback.method != "lvmthin" || config.boot.initrd.services.lvm.enable;
        message = ''
          system.ephemeralRoot.rollback.method = "lvmthin" requires
          boot.initrd.services.lvm.enable = true, which is where the initrd gets lvm2,
          the device-mapper udev rules and the thin-provisioning tools. Without them the
          root volume is never activated.
        '';
      }
      {
        # Constrains every path a mount unit name is derived from to what
        # `utils.escapeSystemdPath` escapes correctly (nixpkgs#515270
        # mis-escapes `.` segments), so each derived name is one systemd
        # actually instantiates. A name that is merely wrong fails an `After=`
        # silently, which is how the rollback's ordering did nothing for a day.
        assertion = lib.all (path: builtins.match "(/[a-zA-Z0-9_-]+)+" path != null) escapedPaths;
        message = ''
          system.ephemeralRoot derives systemd mount unit names from these paths, so
          each must be a simple absolute path whose segments are [a-zA-Z0-9_-]:
          ${concatMapStringsSep "\n" (path: "  ${path}")
            (builtins.filter (path: builtins.match "(/[a-zA-Z0-9_-]+)+" path == null) escapedPaths)}
        '';
      }
      {
        assertion = config.fileSystems ? ${cfg.persistentRoot};
        message = ''
          system.ephemeralRoot needs `fileSystems."${cfg.persistentRoot}"` declared,
          on a filesystem the rollback does not touch.
        '';
      }
      {
        assertion =
          cfg.rollback.method
          == "none"
          || (config.fileSystems.${cfg.persistentRoot}.neededForBoot or false);
        message = ''
          fileSystems."${cfg.persistentRoot}".neededForBoot must be true: the initrd
          seeds /etc/machine-id and /var/lib/nixos into it before stage 2 starts.
        '';
      }
      {
        # See `inInitrd`: the initrd seed has no name service to resolve
        # against, because the root it would read /etc/passwd from was just
        # blanked. Numeric ids would work and are deliberately not offered;
        # nothing has needed a non-root early entry.
        assertion = lib.all (e: e.user == "root" && e.group == "root") initrdEntries;
        message = ''
          system.ephemeralRoot: every `inInitrd` entry must be root-owned. These are not:
          ${concatMapStringsSep "\n" (e: "  ${e.path} (${e.user}:${e.group})")
            (builtins.filter (e: e.user != "root" || e.group != "root") initrdEntries)}
        '';
      }
      {
        # NixOS activation runs inside the initrd, not stage 2
        # (`initrd-nixos-activation.service`, which finishes before
        # `Switching root`), and `setup-etc.pl` materialises every path in the
        # generation's /etc tree as it goes. An /etc entry seeded after that
        # is too late by one step: a bind mount hides the static files
        # activation just wrote, and a symlink `L+` deletes the directory
        # holding them. Both boot and both look fine until something reads the
        # file that is no longer there.
        assertion = lib.all (e: e.inInitrd) etcEntries;
        message = ''
          system.ephemeralRoot: every entry under /etc must set `inInitrd = true`,
          because NixOS activation writes /etc from the initrd and would overwrite or
          be overwritten by a seed that runs later. These do not:
          ${concatMapStringsSep "\n" (e: "  ${e.path}")
            (builtins.filter (e: !e.inInitrd) etcEntries)}
        '';
      }
      {
        # `fileSystems` is defined from `bindMounts` a few lines below, so this
        # can only fail when something downstream replaced the whole attribute
        # set rather than merging into it. `qemu-vm.nix` does exactly that
        # (`mkVMOverride`, priority 10), and the result is a machine that boots
        # normally with a whitelist mounting nothing: the wipe still fires, so
        # the state it was supposed to keep is gone and nothing reports it.
        # Cheap to check here, and the alternative is finding out from a VM
        # that took six minutes to say so.
        assertion = lib.all (path: config.fileSystems ? ${path}) (lib.attrNames cfg.bindMounts);
        message = ''
          system.ephemeralRoot: these whitelist binds are missing from `fileSystems`,
          so something replaced it instead of merging into it. Merge
          `config.system.ephemeralRoot.bindMounts` into whatever assembles it (inside a
          NixOS VM test that is `virtualisation.fileSystems`):
          ${concatMapStringsSep "\n" (path: "  ${path}")
            (builtins.filter (path: !(config.fileSystems ? ${path})) (lib.attrNames cfg.bindMounts))}
        '';
      }
      {
        assertion = userMismatches == [];
        message = ''
          system.ephemeralRoot.users disagrees with users.users, so entries would be
          created under the wrong path or owner:

          ${concatStringsSep "\n" userMismatches}
        '';
      }
      {
        assertion = unwhitelistedStateDirectories == [];
        message = ''
          system.ephemeralRoot: these services declare a StateDirectory whose contents
          this machine wipes on every boot. Add each to `system.ephemeralRoot.entries`
          to keep it, or to `system.ephemeralRoot.ephemeralStateDirectories` to say
          out loud that losing it is fine:

          ${concatMapStringsSep "\n" (dir: "  /var/lib/${dir}") unwhitelistedStateDirectories}
        '';
      }
    ];

    environment.systemPackages = [wipePreview];

    # Only the two methods that name a filesystem. `lvmthin` is a block-layer
    # mechanism: the root LV carries whatever filesystem the install put on it,
    # and nixpkgs already derives support for that from the `fileSystems` entry
    # naming it (nixos/modules/tasks/filesystems.nix). Passing "lvmthin" here
    # would ask for support for a filesystem that does not exist.
    boot.supportedFilesystems =
      optionalAttrs (builtins.elem cfg.rollback.method ["btrfs" "zfs"])
      {${cfg.rollback.method} = true;};

    # What makes a thin pool activatable in the initrd, and it is more than the
    # kernel modules: LVM refuses to activate a thin pool without `thin_check`,
    # and this is what puts that binary there and writes the initrd's
    # /etc/lvm/lvm.conf pointing at it (nixpkgs nixos/modules/tasks/lvm.nix:83).
    # It also adds the dm-thin-pool and dm-snapshot modules, which is why they
    # are not listed again here: one spelling, in the module that owns them.
    services.lvm.boot.thin.enable = mkIf (cfg.rollback.method == "lvmthin") true;

    # THE WHITELIST. `depends` orders each bind after the filesystem it reads
    # from; without it systemd is free to try the bind first and fail the boot
    # on a source that is not mounted yet.
    system.ephemeralRoot.bindMounts =
      lib.genAttrs' bindEntries
      (entry:
        lib.nameValuePair entry.path {
          device = sourceOf entry;
          # `none` is how a bind mount is spelled in fstab. Leaving it unset
          # does not default: nixpkgs raises "option was accessed but has no
          # value defined" from inside `boot.initrd.supportedFilesystems`,
          # several modules away from anything naming this mount.
          fsType = "none";
          options = ["bind"];
          # The initrd entries have to be mounted before NixOS activation, and
          # activation runs in the initrd. `/var/lib/nixos` is the one that
          # matters: activation allocates uids there, so a bind that arrives in
          # stage 2 gets a set of allocations made against the blank root.
          neededForBoot = entry.inInitrd;
          depends = [cfg.persistentRoot];
        });

    fileSystems = cfg.bindMounts;

    # `systemd-machine-id-commit.service` copies a machine id that was
    # generated into a tmpfs back onto real storage. It decides the id is
    # transient with `ConditionPathIsMountPoint=/etc/machine-id`, which a
    # whitelisted /etc/machine-id satisfies for an unrelated reason: it is a
    # bind mount from the persistent filesystem, so the id is already saved and
    # there is nothing to commit. The condition passes, the tool then checks
    # properly, reports "/etc/machine-id is not on a temporary file system" and
    # exits 1. Measured in the VM test on 2026-08-06, on every boot.
    #
    # Masked rather than left to fail. A unit that fails on every boot is one an
    # operator learns to scroll past, and a fleet that has learned to scroll
    # past a failed unit scrolls past the next one too.
    systemd.suppressedSystemUnits =
      lib.optional (lib.any (entry: entry.path == "/etc/machine-id") bindEntries)
      "systemd-machine-id-commit.service";

    # Stage 2 seed, ordered before `local-fs-pre.target`: the point systemd
    # reserves for work that has to happen before the fstab mounts it
    # generated are tried. An ordinary service running later would be too late
    # by exactly one boot, because the mounts fail first.
    systemd.services.ephemeral-root-seed = {
      description = "Create whitelisted sources on the persistent filesystem";
      wantedBy = ["local-fs-pre.target"];
      before = ["local-fs-pre.target"];
      unitConfig = {
        DefaultDependencies = "no";
        RequiresMountsFor = [cfg.persistentRoot];
      };
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = "${config.systemd.package}/bin/systemd-tmpfiles --create ${allRules}";
      };
    };

    boot.initrd.systemd = {
      # `--root=/sysroot` reinterprets every path in the rules file under the
      # mounted root, which is where the persistent filesystem is at this
      # point. Only the `inInitrd` subset is applied: the rest names users
      # that cannot be resolved yet.
      services.ephemeral-root-seed = mkIf (initrdEntries != []) {
        description = "Create early whitelisted sources on the persistent filesystem";
        wantedBy = ["initrd-fs.target"];
        before = ["initrd-fs.target"] ++ initrdBindMountUnits;
        after = [persistentInitrdMountUnit];
        requires = [persistentInitrdMountUnit];
        unitConfig.DefaultDependencies = "no";
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          ExecStart = "systemd-tmpfiles --create --root=/sysroot ${initrdRules}";
        };
      };

      services.ephemeral-root-rollback = mkIf (cfg.rollback.method != "none") {
        description = "Return the root filesystem to its blank snapshot";
        wantedBy = ["initrd-root-fs.target"];

        # THE ORDERING THAT KEEPS THE ROOT MOUNT'S JOB ALIVE, and it is not
        # `Before=sysroot.mount` alone. Measured 2026-08-06 by
        # tests/ephemeral-root-lvmthin-vm.nix, on the first boot the `lvmthin`
        # method ever had; the initrd stalled forever with no failed unit and
        # nothing on the console.
        #
        # `lvmthin` renames the root LV aside and creates a new one under the
        # old name, so the root's device node is destroyed and re-created. What
        # that costs is nothing to do with the device unit itself, which comes
        # back: the probes in that test recorded `dev-vg0-root.device` as
        # active on both sides, on `dm-5` before and `dm-7` after. It is
        # `systemd-fsck-root.service`. The fstab generator gives it
        # `BindsTo=<root>.device`, and gives `sysroot.mount`
        # `Requires=<root>.device systemd-fsck-root.service`. With no ordering
        # against it the fsck wins the race, runs, and is *active* when the
        # rename pulls the device out from under it:
        #
        #   systemd[1]: Finished File System Check on /dev/vg0/root.
        #   ...
        #   ephemeral-root-rollback-start[185]: Renamed "root" to "old-root-..."
        #   systemd[1]: systemd-fsck-root.service: Deactivated successfully.
        #   systemd[1]: Stopped File System Check on /dev/vg0/root.
        #
        # and stopping a unit that `sysroot.mount` requires takes the mount's
        # queued job with it -- and with it `initrd-root-fs.target`,
        # `initrd.target`, and every job in the boot. The observed job list went
        # from nineteen entries to one (this service's own), after which systemd
        # printed "Startup finished" with nothing left to do and sat there. No
        # unit failed, so nothing reported it.
        #
        # Ordering before the fsck means the fsck has not run when the device
        # goes away, so there is no active unit to stop and no job to cancel;
        # the fsck and the mount are both still waiting on
        # `After=<root>.device` and simply run against the LV this service
        # creates. `systemd-fsck-root.service` is the name systemd's fstab
        # generator special-cases the root to, and `Before=` a unit no
        # generator instantiated (a root with `passno = 0`) is satisfied
        # trivially, so this costs nothing where it does not apply.
        #
        # With the line below, the same probe on the same configuration shows
        # `systemd-fsck-root.service start running` and `sysroot.mount start
        # waiting` still in the job list after the rollback, the fsck then
        # passing on the new LV ("/dev/mapper/vg0-root: clean"), and the mount
        # landing on `dm-7` -- the node the rollback created, not the one it
        # destroyed.
        #
        # Only `lvmthin` needs it. `btrfs` renames a subvolume inside a mounted
        # filesystem and `zfs` rolls a dataset back; neither takes a device
        # node away from anything.
        before =
          ["sysroot.mount"]
          ++ lib.optional (cfg.rollback.method == "lvmthin") "systemd-fsck-root.service";

        # THE HIBERNATION GUARD, and it is not the obvious one.
        #
        # Resuming from hibernation restores a memory image describing a
        # filesystem as it was before the machine slept. Wiping the root and
        # then resuming hands a live kernel a disk that matches nothing it
        # believes, which corrupts both.
        #
        # The obvious guard, `ConditionKernelCommandLine = ["!resume"]`, is
        # wrong: NixOS puts `resume=` on the command line whenever a resume
        # device is configured, not only when an image is actually present,
        # so that condition disables the wipe permanently on any machine that
        # could hibernate. Ordering after the resume unit is the mechanism
        # instead. If an image exists that unit jumps into it and this service
        # never runs; if it does not, the unit exits and the wipe proceeds.
        # `systemd-hibernate-resume.service` is in the initrd's default unit
        # set (nixpkgs nixos/modules/system/boot/systemd/initrd.nix:102), and
        # `After=` on a unit no generator instantiated is satisfied trivially,
        # so this costs nothing when hibernation is off.
        #
        # WHY `initrd-root-device.target` RATHER THAN THE DEVICE'S OWN UNIT.
        # The device is the thing actually being waited for, and naming its
        # unit means deriving `dev-disk-by\x2dlabel-ephemeral.device` from a
        # path. An earlier version of this line escaped only `/` and produced
        # `dev-disk-by-label-ephemeral.device`, which no generator instantiates.
        # `After=` on a unit that does not exist is satisfied immediately, so
        # the ordering silently did nothing. It passed anyway
        # until unrelated timing moved, then the rollback ran before the disk
        # appeared and failed with "Could not statfs" (2026-08-06).
        #
        # `initrd-root-device.target` is the point systemd already publishes
        # for exactly this, so there is no name to spell and no spelling to get
        # wrong. That is the reason to prefer it, not brevity.
        after = [
          "systemd-hibernate-resume.service"
          "initrd-root-device.target"
        ];

        unitConfig = {
          DefaultDependencies = "no";

          # THE GUARD THAT MAKES A SECOND RUN HARMLESS RATHER THAN FATAL.
          #
          # A btrfs rename does not change a subvolume's id, so renaming a
          # subvolume that is already mounted leaves the mount live and moves
          # it: the running root silently becomes `/old_roots/<stamp>` and the
          # fresh root nothing is using is what the next boot gets. Measured on
          # 2026-08-06 before this line existed, as `/ / btrfs
          # subvolid=260,subvol=/old_roots/2026-08-06T10:07:05` in the test
          # VM's `/proc/mounts`.
          #
          # `RemainAfterExit` below is what stops the re-run that caused it,
          # and this is the check that the re-run cannot hurt anything if it
          # ever happens by another route. Ordering alone would not do it:
          # `Before=sysroot.mount` constrains the first activation and says
          # nothing about a later one.
          #
          # `lvmthin` has the same shape for the same reason: an `lvrename`
          # keeps the device-mapper device, so renaming a mounted root volume
          # would move the live root aside rather than fail.
          ConditionPathIsMountPoint = "!/sysroot";
        };

        serviceConfig = {
          Type = "oneshot";
          # Without this the unit is inactive the moment it succeeds, so the
          # switch-root isolate at the end of the initrd pulls it in and runs
          # it a second time, after `sysroot.mount`. Every initrd oneshot
          # that establishes state needs it; `DefaultDependencies=no` is what
          # removes the ordering that would otherwise have hidden the problem.
          RemainAfterExit = true;
        };

        # WHY SHELL, given the repo writes branching logic in Rust. A Rust
        # binary here would have to be pulled into the initrd closure by hand,
        # and this has to be readable in one screen by whoever is deciding
        # whether to trust it with a root filesystem. `set -euo pipefail` is
        # what makes a partial failure a failed boot into the emergency shell
        # rather than a silent boot onto a half-rolled-back root; without it
        # this is exactly the silent fallback the repo forbids.
        script = ''
          set -euo pipefail
          ${rollbackScripts.${cfg.rollback.method}}
        '';
      };

      # The tools each script calls, named where the script is. lvm2 is also
      # added by nixpkgs' lvm module through `boot.initrd.services.lvm.enable`,
      # which the assertion above requires; it is repeated because a reader
      # checking whether `lvcreate` is in the initrd should find the answer
      # next to the unit that runs it.
      #
      # gawk because the `lvmthin` prune pipes through `awk`, and a systemd
      # initrd has no awk: `boot.initrd.systemd.initrdBin` defaults to coreutils
      # + systemd (+ kmod, + bashInteractive) and `extraBin` adds only
      # less/mount/umount/fsck. Without this the prune died with
      #
      #   ephemeral-root-rollback-start[213]: .../ephemeral-root-rollback-start:
      #     line 31: awk: command not found
      #   systemd[1]: ephemeral-root-rollback.service: Main process exited,
      #     code=exited, status=127/n/a
      #
      # measured 2026-08-06 by tests/ephemeral-root-lvmthin-vm.nix on the first
      # boot this method ever had. The `lvrename` and the `lvcreate` above it
      # had already succeeded, so the wipe itself worked and only the retention
      # never ran: every displaced root would have accumulated in the thin pool
      # until it filled. On a machine that got past the ordering bug documented
      # on `before` above, that is what this would have looked like -- an initrd
      # unit's failure does not show up in stage 2's `systemctl --failed`, so
      # nothing would have said so.
      #
      # The `zfs` method's prune uses awk too and has never needed a line here
      # because nixpkgs' zfs module puts awk in the initrd itself
      # (nixos/modules/tasks/filesystems/zfs.nix, `extraBin.awk`). Nothing does
      # that for lvm2.
      initrdBin =
        lib.optional (cfg.rollback.method == "btrfs") pkgs.btrfs-progs
        ++ lib.optionals (cfg.rollback.method == "lvmthin") [pkgs.lvm2 pkgs.gawk];

      # The initrd has its own store holding only what is listed here, and the
      # seed reads this file from inside it. Leaving it out does not fail at
      # eval or build: the unit runs and `systemd-tmpfiles` reports "Failed to
      # read ...ephemeral-root-initrd.conf: No such file or directory", so the
      # whitelist is simply not seeded and the machine boots without the state
      # that was supposed to survive. Measured 2026-08-06.
      #
      # Ordering after the store mount would be the other fix and is worse:
      # `/sysroot/nix/store` mounts as part of `initrd-fs.target`, which is the
      # same target this seed has to precede.
      storePaths = lib.optional (initrdEntries != []) initrdRules;
    };
  };
}
