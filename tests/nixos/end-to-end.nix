{ azbincachePackage }:
{ pkgs, ... }:

let
  targetPkg = pkgs.hello;
  otherPkg = pkgs.cowsay;
in
{
  name = "azbincache-end-to-end";

  nodes.machine =
    { config, lib, pkgs, ... }:
    {
      virtualisation.memorySize = 4096;
      virtualisation.diskSize = 8192;

      environment.systemPackages = [
        azbincachePackage
        pkgs.curl
        pkgs.jq
      ];

      # Make the test packages (and their closures) available in the VM store
      # without any network access, so `azbincache push` has real paths to upload.
      system.extraDependencies = [
        targetPkg
        otherPkg
      ];

      services.nginx = {
        enable = true;
        additionalModules = [ pkgs.nginxModules.dav ];
        virtualHosts."cache" = {
          listen = [
            {
              addr = "127.0.0.1";
              port = 8080;
            }
          ];
          root = "/var/lib/azbincache-www";
          locations."/" = {
            extraConfig = ''
              autoindex on;
              dav_methods PUT DELETE;
              dav_ext_methods PROPFIND OPTIONS;
              create_full_put_path on;
              client_max_body_size 0;
            '';
          };
        };
      };

      systemd.services.nginx.serviceConfig.ReadWritePaths = [ "/var/lib/azbincache-www" ];

      systemd.tmpfiles.rules = [
        "d /var/lib/azbincache-www 0755 nginx nginx -"
      ];

      nix.settings.experimental-features = [
        "nix-command"
        "flakes"
      ];
    };

  testScript = ''
    machine.start()
    machine.wait_for_unit("nginx.service")
    machine.wait_for_open_port(8080)

    # Provide a known signing key and derive its public counterpart.
    machine.succeed(
        "nix-store --generate-binary-cache-key azbincache-test-1 /root/sk /root/pk"
    )
    pubkey = machine.succeed("cat /root/pk").strip()

    # A store path with a dependency closure, supplied via the VM's store.
    target = "${targetPkg}"

    # 1. PUSH the full closure to the HTTP-backed cache.
    machine.succeed(
        f"azbincache push --to http://127.0.0.1:8080 "
        f"--signing-key-file /root/sk --no-upstream-skip "
        f"--commit commitA --commit-time 100 --host machine {target}"
    )

    # The cache must expose nix-cache-info, a narinfo, and a nar.
    machine.succeed("curl -fsS http://127.0.0.1:8080/nix-cache-info | grep StoreDir")
    store_hash = target.split("/")[-1].split("-")[0]
    machine.succeed(f"curl -fsS http://127.0.0.1:8080/{store_hash}.narinfo | grep '^Sig: azbincache-test-1:'")

    # 2. READ: a real nix substitutes the whole closure from our cache, with
    #    signature enforcement against the public key we generated.
    machine.succeed(
        f"nix copy --from http://127.0.0.1:8080 --to /root/teststore "
        f"--option require-sigs true "
        f"--option trusted-public-keys '{pubkey}' {target}"
    )
    machine.succeed(f"test -e /root/teststore/{target}/bin/hello")
    machine.succeed(f"/root/teststore/{target}/bin/hello | grep 'Hello, world'")

    # 3. A WRONG key must be REJECTED (proves signatures are actually checked).
    machine.fail(
        f"nix copy --from http://127.0.0.1:8080 --to /root/badstore "
        f"--option require-sigs true "
        f"--option trusted-public-keys 'wrong-1:0000000000000000000000000000000000000000000=' {target}"
    )

    # 4. GC: push a second commit, keep only the most recent, verify the first
    #    commit's unique paths are removed and the second commit survives.
    other = "${otherPkg}"
    machine.succeed(
        f"azbincache push --to http://127.0.0.1:8080 "
        f"--signing-key-file /root/sk --no-upstream-skip "
        f"--commit commitB --commit-time 200 --host machine {other}"
    )

    before = int(machine.succeed("ls /var/lib/azbincache-www/*.narinfo | wc -l").strip())
    assert before >= 2, f"expected >=2 narinfos before gc, got {before}"

    machine.succeed("azbincache gc --to http://127.0.0.1:8080 --keep-commits 1")

    # hello (commitA) must be gone; cowsay (commitB) must remain.
    machine.fail(f"test -e /var/lib/azbincache-www/{store_hash}.narinfo")
    other_hash = other.split("/")[-1].split("-")[0]
    machine.succeed(f"test -e /var/lib/azbincache-www/{other_hash}.narinfo")

    # 5. The surviving path is still substitutable end-to-end after GC.
    machine.succeed(
        f"nix copy --from http://127.0.0.1:8080 --to /root/store2 "
        f"--option require-sigs true "
        f"--option trusted-public-keys '{pubkey}' {other}"
    )
  '';
}
