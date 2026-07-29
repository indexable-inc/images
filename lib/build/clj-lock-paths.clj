;; Resolve the classpath entries a lock's git libraries contribute, by reading
;; the deps.edn files inside the fetched checkouts.
;;
;; deps-lock.json records where a git dependency was fetched from but nothing
;; about what inside it is code. Two separate facts live in EDN instead:
;;
;;   :deps/root  in the CONSUMING project's deps.edn, selecting a subdirectory
;;               of the checkout (a monorepo like Biff pins `libs/core`,
;;               `libs/fx`, ... out of one commit).
;;   :paths      in that subdirectory's own deps.edn, listing its source and
;;               resource directories. tools.deps defaults it to ["src"].
;;
;; A library reached through :local/root is invisible to the lock -- Biff's
;; com.biffweb/sqlite pulls in ../graph that way -- so this walks those edges
;; too, or the classpath is missing code that the lock gave no hint about.
;;
;; Usage: bb clj-lock-paths.clj <plan.json> <project-deps.edn>
;; Prints one classpath entry per line, in walk order, deduplicated.
(require '[babashka.fs :as fs]
         '[cheshire.core :as json]
         '[clojure.edn :as edn])

(defn die [& parts]
  (binding [*out* *err*]
    (println (apply str "clj-lock: " parts)))
  (System/exit 1))

(defn read-edn
  "deps.edn at `dir`, or a loud failure naming the library it belongs to."
  [dir {:keys [lib rev]}]
  (let [file (fs/path dir "deps.edn")]
    (when-not (fs/directory? dir)
      (die "git library " lib " at rev " rev " has no directory " (str dir)
           "; check its :deps/root, or the :local/root that reached it"))
    (when-not (fs/regular-file? file)
      (die "git library " lib " at rev " rev " has no deps.edn at " (str file)
           "; every git dependency root must carry one"))
    (edn/read-string (slurp (str file)))))

(defn walk
  "Classpath entries contributed by `dir`, then by every :local/root it names.
  `seen` breaks the diamonds a monorepo produces (fx and ring both reach core)."
  [dir library seen]
  (let [dir (str (fs/normalize dir))]
    (if (contains? @seen dir)
      []
      (do
        (swap! seen conj dir)
        (let [deps (read-edn dir library)
              own (map #(str (fs/path dir %)) (:paths deps ["src"]))
              locals (keep (fn [[_ coord]] (:local/root coord)) (:deps deps))]
          (into (vec own)
                (mapcat #(walk (fs/path dir %) library seen) locals)))))))

(let [[plan-file project-deps-file] *command-line-args*
      plan (json/parse-string (slurp plan-file) true)
      ;; :deps/root is ours to declare, so it comes from the consuming project.
      project-deps (:deps (edn/read-string (slurp project-deps-file)))
      seen (atom #{})
      git-entries
      (vec (mapcat (fn [{:keys [lib checkout] :as library}]
                     ;; A transitively locked git library is absent from our
                     ;; deps.edn; tools.deps then roots it at the checkout.
                     (let [root (:deps/root (get project-deps (symbol lib)) "")]
                       (walk (fs/path checkout root) library seen)))
                   (:gitLibraries plan)))]
  (run! println (concat git-entries (:jars plan))))
