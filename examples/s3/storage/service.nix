{pkgs, ...}: let
  # DEMO credentials, baked into the store on purpose so the example runs
  # as-is. For anything real, point `configFile` at a runtime secret file
  # (e.g. /run/secrets/seaweedfs-s3.json) so keys never enter the store.
  demoAccessKey = "ix-demo-access-key";
  demoSecretKey = "ix-demo-secret-key";
  demoS3Config = (pkgs.formats.json {}).generate "seaweedfs-s3-demo.json" {
    identities = [
      {
        name = "demo";
        credentials = [
          {
            accessKey = demoAccessKey;
            secretKey = demoSecretKey;
          }
        ];
        actions = [
          "Admin"
          "Read"
          "Write"
          "List"
          "Tagging"
        ];
      }
    ];
  };
in {
  # The module owns the port claim, firewall opening, and the `/healthz`
  # readiness check; enabling it is all the example needs.
  services.ix-seaweedfs = {
    enable = true;
    configFile = demoS3Config;
  };

  # A small S3 client for the round-trip in the README and ad-hoc pokes.
  environment.systemPackages = [pkgs.s5cmd];

  # Surface the demo endpoint and credentials to anyone shelled in, so the
  # round-trip below is copy-paste ready. Production deployments would not
  # ship credentials this way.
  # Demo only: this puts the secret key in the guest's global
  # /etc/set-environment, readable by every user and process in the VM.
  # Never export real keys this way; load them from a secret file instead.
  environment.variables = {
    # `s5cmd` reads S3_ENDPOINT_URL; the AWS CLI reads AWS_ENDPOINT_URL.
    # Set both so either client works against the local gateway unflagged.
    S3_ENDPOINT_URL = "http://127.0.0.1:8333";
    AWS_ENDPOINT_URL = "http://127.0.0.1:8333";
    AWS_ACCESS_KEY_ID = demoAccessKey;
    AWS_SECRET_ACCESS_KEY = demoSecretKey;
    AWS_REGION = "us-east-1";
  };
}
