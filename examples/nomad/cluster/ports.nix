# One statement of every port in the cluster. 4646-4648 are nomad's
# defaults, restated here only so expose entries and endpoint references
# share a single source.
{
  http = 4646;
  rpc = 4647;
  serf = 4648;
  app = 8080;
}
