{
  bundledSource,
  bundledTestPython,
  importTest,
  lib,
  pkgs,
}: let
  # Native macOS places & geocoding: places near a point (MapKit `MKLocalSearch`)
  # and geocoding both ways (CoreLocation `CLGeocoder`), all returned as polars
  # frames. Pure Python over the bundled pyobjc CoreLocation/MapKit; its async
  # bridge drains the main run loop cooperatively so the frameworks' main-thread
  # completion handlers fire without wedging the kernel's event loop. macOS-only
  # (the module raises off Darwin).
  mapsPythonSource = bundledSource {
    name = "ix-mcp-maps-python-source";
    path = ./.;
  };
  mapsModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-maps-python-module"
    {
      strictDeps = true;
      meta.description = "Native macOS maps/location (MapKit + CoreLocation) bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/maps"
      mkdir -p "$site"
      cp -r ${mapsPythonSource}/maps/. "$site/"
    ''
  );
  mapsBundled = importTest [mapsModule] "maps" "import maps, MapKit; print('maps-ok', all(callable(getattr(maps, n)) for n in ('nearby', 'geocode', 'reverse_geocode')), callable(MapKit.MKLocalSearch.alloc))";
  # The maps module: pure-helper checks that need no network or location
  # permission (the nix sandbox has neither). Exercises the radius->region span
  # math (incl. the latitude cosine correction) and the polars schema shapes, and
  # confirms the public coroutines and MapKit binding are present.
  mapsTestPy = pkgs.writeText "ix-mcp-maps-test.py" ''
    # python
    import inspect
    import math

    import polars as pl

    import maps
    import MapKit

    # Public coroutine surface is callable and async.
    for name in ("nearby", "geocode", "reverse_geocode"):
        fn = getattr(maps, name)
        assert inspect.iscoroutinefunction(fn), name

    # MapKit binding loads (the place-search class is present).
    assert callable(MapKit.MKLocalSearch.alloc), "MKLocalSearch missing"

    # region(): span is the full width/height, so twice the radius in degrees;
    # latitude degrees are constant, longitude degrees shrink with cos(latitude).
    (clat, clng), (lat_delta, lng_delta) = maps._region(0.0, 0.0, 1000.0)
    assert (clat, clng) == (0.0, 0.0)
    assert math.isclose(lat_delta, 2000.0 / 111320.0, rel_tol=1e-9), lat_delta
    assert math.isclose(lng_delta, lat_delta, rel_tol=1e-9), (lat_delta, lng_delta)
    # At 60 deg latitude cos=0.5, so longitude span is ~2x the latitude span.
    (_c2, (lat60, lng60)) = maps._region(60.0, 0.0, 1000.0)
    assert math.isclose(lng60 / lat60, 2.0, rel_tol=1e-6), (lat60, lng60)

    # Schemas: nearby is the placemark schema plus the POI columns.
    placemark = set(maps._placemark_schema(pl))
    nearby = set(maps._nearby_schema(pl))
    assert {"name", "latitude", "longitude", "country"} <= placemark, placemark
    assert nearby - placemark == {"category", "phone"}, nearby - placemark

    print("maps-ok")
  '';
  mapsTestPython = bundledTestPython [mapsModule];
  mapsSmoke =
    pkgs.runCommand "ix-mcp-maps-smoke"
    {
      nativeBuildInputs = [mapsTestPython];
      strictDeps = true;
    }
    ''
      ${lib.getExe mapsTestPython} ${mapsTestPy} >stdout 2>stderr || {
        echo "ix-mcp maps smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'maps-ok' stdout || {
        echo "ix-mcp maps smoke did not confirm the maps module:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
in {
  module = mapsModule;
  darwinOnly = true;
  tests = lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
    inherit
      mapsBundled
      mapsSmoke
      ;
  };
}
