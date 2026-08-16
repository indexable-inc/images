#include "nix/store/globals.hh"
#include "nix/store/store-open.hh"
#include "nix/store/store-api.hh"
#include "nix/store/derivations.hh"
#include "nix/store/realisation.hh"

#include <iostream>
#include <string>

using namespace nix;

/* Take a realisation out of one store and register it in another, and do
   nothing else.

   Nothing else is the point. Every command-line route into
   `registerDrvOutput` happens to call `isValidPath` on the output path on its
   way there -- `copyPaths` to decide what to send, the derivation goal to
   decide whether it must build -- and for a `local-overlay` store that call
   copies the lower layer's registration up as a side effect. So the store
   always has the row by the time the realisation insert needs it, and the
   missing copy-up in `registerDrvOutput` never shows.

   A long-lived process does not get that. `Store::isValidPath` answers from
   the path-info cache, and `queryPathInfoUncached` fills that cache from the
   lower store without registering anything, so one earlier `queryPathInfo`
   anywhere in the process -- a closure walk, `queryMissing`, a substituter
   check -- turns the copy-up off for the rest of its life. `--warm-cache`
   puts the store in that state in one step. */
int main(int argc, char ** argv)
{
    try {
        if (argc < 5 || argc > 6) {
            std::cerr << "Usage: " << argv[0] << " <src-store> <dst-store> <drv-path> <output-name> [--warm-cache]\n";
            return 1;
        }

        std::string srcUri = argv[1];
        std::string dstUri = argv[2];
        std::string drvPathStr = argv[3];
        std::string outputName = argv[4];

        bool warmCache = false;
        if (argc == 6) {
            if (std::string(argv[5]) != "--warm-cache") {
                std::cerr << "unknown option '" << argv[5] << "'\n";
                return 1;
            }
            warmCache = true;
        }

        initLibStore();

        auto srcStore = openStore(srcUri);
        auto dstStore = openStore(dstUri);

        auto drvPath = srcStore->parseStorePath(drvPathStr);
        auto drvHashes = staticOutputHashes(*srcStore, srcStore->readDerivation(drvPath));
        auto drvHash = drvHashes.find(outputName);
        if (drvHash == drvHashes.end())
            throw Error("derivation '%s' has no output '%s'", drvPathStr, outputName);

        DrvOutput id{drvHash->second, outputName};
        auto realisation = srcStore->queryRealisation(id);
        if (!realisation)
            throw Error("the source store has no realisation for '%s'", id.to_string());

        if (warmCache)
            dstStore->queryPathInfo(realisation->outPath);

        dstStore->registerDrvOutput(Realisation{*realisation, id});

        std::cout << dstStore->printStorePath(realisation->outPath) << "\n";
        return 0;

    } catch (const std::exception & e) {
        std::cerr << "Error: " << e.what() << "\n";
        return 1;
    }
}
