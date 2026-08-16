# Git submodules

This is an aspirational document that describes how jj _will_ support Git
submodules. Readers are assumed to have some familiarity with Git and Git
submodules.

This document is a work in progress; submodules are a big feature, and relevant
details will be filled in incrementally.

## Implementation status

Nothing in the roadmap below has been implemented. None of the Phase 1 outcomes
work, and jj does not read `.gitmodules` at all.

What exists is the handling needed to leave a repository that contains
submodules intact. `TreeValue::GitSubmodule` holds a gitlink's commit id, so a
tree with a submodule in it can be read from and written back to the Git backend
without losing the entry. The working copy leaves submodule directories alone:
checking out a tree with a gitlink creates the directory, prints `ignoring git
submodule at <path>`, and does not populate or snapshot its contents. A
`SubmoduleStore` trait and a `.jj/repo/submodule_store` directory exist, but the
trait has one method and it returns the store's name.

## Objective

This proposal aims to replicate the workflows users are used to with Git
submodules, e.g.:

- Cloning submodules
- Making new submodule commits and updating the superproject
- Fetching and pushing updates to the submodule's remote
- Viewing submodule history

When it is convenient, this proposal will also aim to make submodules easier to
use than Git's implementation.

### Non-goals

- Non-Git 'submodules' (e.g. native jj submodules, other VCSes)
- Non-Git backends (e.g. Google internal backend)
- Changing how Git submodules are implemented in Git

## Background

We mainly want to support Git submodules for feature parity, since Git
submodules are a standard feature in Git and are popular enough that we have
received user requests for them. Secondarily (and distantly so), Git submodules
are notoriously difficult to use, so there is an opportunity to improve the UX
over Git's implementation.

### Intro to Git Submodules

[Git submodules](https://git-scm.com/docs/gitsubmodules) are a feature of Git
that allow a repository (submodule) to be embedded inside another repository
(the superproject). Notably, a submodule is a full repository, complete with its
own index, object store and ref store. It can be interacted with like any other
repository, regardless of the superproject.

In a superproject commit, submodule information is captured in two places:

- A `gitlink` entry in the commit's tree, where the value of the `gitlink` entry
  is the submodule commit id. This tells Git what to populate in the working
  tree.

- A top level `.gitmodules` file. This file is in Git's config syntax and
  entries take the form `submodule.<submodule-name>.*`. These include many
  settings about the submodules, but most importantly:

  - `submodule<submodule-name>.path` contains the path from the root of the tree
    to the `gitlink` being described.

  - `submodule<submodule-name>.url` contains the url to clone the submodule
    from.

In the working tree, Git notices the presence of a submodule by the `.git` entry
(signifying the root of a Git repository working tree). This is either the
submodule's actual Git directory (an "old-form" submodule), or a `.git` file
pointing to `<superproject-git-directory>/modules/<submodule-name>`. The latter
is sometimes called the "absorbed form", and is Git's preferred mode of
operation.

## Roadmap

Git submodules should be implemented in an order that supports an increasing set
of workflows, with the goal of getting feedback early and often. When support is
incomplete, jj should not crash, but instead provide fallback behavior and warn
the user where needed.

The goal is to land good support for pure Jujutsu workspaces, while colocated
workspaces will be supported when convenient.

This section should be treated as a set of guidelines, not a strict order of
work.

### Phase 1: Readonly submodules

This includes work that inspects submodule contents but does not create new
objects in the submodule. This requires a way to store submodules in a jj
repository that supports readonly operations.

#### Outcomes

- Submodules can be cloned anew
- New submodule commits can be fetched
- Submodule history and branches can be viewed
- Submodule contents are populated in the working copy
- Superproject gitlink can be updated to an existing submodule commit
- Conflicts in the superproject gitlink can be resolved to an existing submodule
  commit

### Phase 2: Snapshotting new changes

This allows a user to write new contents to a submodule and its remote.

#### Outcomes

- Changes in the working copy can be recorded in a submodule commit
- Submodule branches can be modified
- Submodules and their branches can be pushed to their remote

### Phase 3: Merging/rebasing/conflicts

This allows merging and rebasing of superproject commits in a content-aware way
(in contrast to Git, where only the gitlink commit ids are compared), as well as
workflows that make resolving conflicts easy and sensible.

This can be done in tandem with Phase 2, but will likely require a significant
amount of design work on its own.

#### Outcomes

- Merged/rebased submodules result in merged/rebased working copy content
- Merged/rebased working copy content can be committed, possibly by creating
  sensible merged/rebased submodule commits
- Merge/rebase between submodule and non-submodule gives a sensible result
- Merge/rebase between submodule A and submodule B gives a sensible result

### Phase ?: An ideal world

I.e. outcomes we would like to see if there were no constraints whatsoever.

- Rewriting submodule commits rewrites descendants correctly and updates
  superproject gitlinks.
- Submodule conflicts automatically resolve to the 'correct' submodule commits,
  e.g. a merge between superproject commits creating a merge of the submodule
  commits.
- Nested submodules are as easy to work with as non-nested submodules.
- The operation log captures changes in the submodule.

## Design

### Guiding principles

These principles exist so that submodule behavior is coherent and so that users
can predict it, especially where jj diverges from Git.

#### Submodules are not standalone repositories

In Git, a submodule is a standalone repository, and can be used without the
superproject knowing. That flexibility is not worth its cost.

A submodule used on its own can invalidate something the superproject relies
on. `git gc` in a submodule deletes objects that no submodule ref reaches, but a
superproject commit can reach a commit that no submodule ref does, so the
collection can delete objects the superproject needs (see [this StackOverflow
question][gc-question]). Git also changes which repository a command applies to
based on where in the working tree it is run, which [confuses
users][cwd-confusion].

[gc-question]: https://stackoverflow.com/questions/31640270/will-git-garbage-collect-commit-in-submodule-referred-to-by-a-top-level-reposito
[cwd-confusion]: https://github.com/jj-vcs/jj/issues/494#issuecomment-1404338917

A submodule exists to be integrated with a superproject; without that it could
be a separate clone instead. So jj will require that every interaction with a
submodule is initiated from the superproject.

#### Commands involve submodules by default

Submodules should be part of the ordinary workflow, not something the user has
to ask for. `jj git clone` should clone a reasonable set of submodules, and
updating the working copy in the superproject should update the submodule
working copies too. There is no `--recurse-submodules` flag to remember, unlike
Git, where forgetting it is a common mistake.

Submodules will sometimes have to be excluded, for example when a submodule's
remote has deleted the commits the superproject refers to. jj should expect
that, recover from it, and tell the user what it did. Manual submodule
management is for exceptional cases only.

#### Submodules are globally managed

In a jj or a Git repository, objects are reusable because they live in a store
that is independent of the working copy. Submodules should work the same way:
the submodule store is the source of truth, and the working copy is at best a
hint.

The consequence is that the working copy is expected to disagree with the store.
Submodules may be missing, their commits may not have been fetched, and their
configuration may differ at different points in history. Git assumes the two
agree and behaves badly when they do not; jj should handle the missing cases.

For that to make sense to users, managing the store and reconciling it with the
working copy has to be easy. Candidates are prompting the user to update
submodules when the working copy changes, reporting that the working copy is out
of sync, syncing the store from `.gitmodules`, and letting the user add, inspect
and remove submodules directly. Which of these jj will offer is not decided.

### Storing submodules

Each Git submodule will be stored as a full jj repo with its own operation log.
jj will interact with a submodule only as a whole unit; it will not query the
submodule's commit backend directly. All interaction is initiated from the
superproject, so that jj can keep the superproject and the submodule consistent
with each other.

Two alternatives were rejected. Storing submodules in the main Git backend under
`.git/modules`, as Git does, was rejected because the operation log rework it
needs does not carry over to non-Git backends. Storing each submodule as an
alternate commit backend was rejected because it also needs an operation log
rework, this time for multiple commit backends, and there is no obvious way to
represent a nested submodule's relationship to its superproject.

Two things this leaves open: how a recursive fetch should behave when a newly
fetched superproject commit references a submodule commit that has not been
fetched, and colocated Git workspaces, which need a change to Git itself before
Git can find a submodule stored this way.

See [./git-submodule-storage.md](./git-submodule-storage.md) for the use cases
the decision was made against and for the full description of the alternatives.

### Snapshotting new submodule changes

TODO

### Merging/rebasing with submodules

TODO
