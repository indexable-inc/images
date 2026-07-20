# Walking-skeleton consumer of the withUsage seam (index#3802): git-log-pretty
# rewrapped so every invocation lands in the local usage store. Tree-wide
# default-on wrapping is a follow-up; this package proves the seam end to end.
{
  withUsage,
  repoPackages,
  ...
}:
withUsage repoPackages.git-log-pretty {
  id = "git-log-pretty";
}
