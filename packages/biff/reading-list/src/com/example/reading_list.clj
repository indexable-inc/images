(ns com.example.reading-list
  (:require [clojure.string :as str]
            [com.biffweb.core :as biff.core]
            [com.biffweb.fx :as biff.fx]
            [com.biffweb.ring :as biff.ring :refer [defroute]]
            [com.biffweb.sqlite :as biff.sqlite])
  (:import [java.net URI URISyntaxException])
  (:gen-class))

(def max-title-length 200)
(def max-url-length 2048)

(defn trim-to-nil [s]
  (when (string? s)
    (not-empty (str/trim s))))

(defn normalize-title [s]
  (when-some [title (trim-to-nil s)]
    (when (<= (count title) max-title-length)
      title)))

(defn normalize-http-url [s]
  (when-some [url (trim-to-nil s)]
    (when (<= (count url) max-url-length)
      (try
        (let [uri (URI. url)
              scheme (some-> (.getScheme uri) str/lower-case)]
          (when (and (#{"http" "https"} scheme)
                     (some? (.getHost uri))
                     (nil? (.getUserInfo uri)))
            url))
        (catch URISyntaxException _
          nil)))))

(defn request-param [params k]
  (or (get params k) (get params (name k))))

(defn read-cookie-secret [path]
  (when-not (some? path)
    (throw (ex-info "COOKIE_SECRET_FILE is required" {})))
  (or (trim-to-nil (slurp path))
      (throw (ex-info "COOKIE_SECRET_FILE must contain a non-blank secret"
                      {:path path}))))

(def columns
  {:link/id         {:type :uuid :primary-key true}
   :link/title      {:type :text :required true}
   :link/url        {:type :text :required true :unique true}
   :link/created-at {:type :inst :required true :index true}})

;; This is intentionally a single-user example. The interesting boundary is
;; explicit: replacing this function with identity-aware rules is the one place
;; writes become authorized when an authentication module is introduced.
(defn authorize-local-write [_ctx _diff]
  true)

(defn links-query []
  {:select [:link/id :link/title :link/url :link/created-at]
   :from :link
   :order-by [[:link/created-at :desc]]})

(defn page [request links]
  [:html
   [:head
    [:meta {:charset "utf-8"}]
    [:meta {:name "viewport" :content "width=device-width, initial-scale=1"}]
    [:title "Reading List"]]
   [:body {:style "font-family: system-ui; max-width: 46rem; margin: 3rem auto; padding: 0 1rem"}
    [:h1 "Reading List"]
    [:p "A deliberately small Biff 2 / SQLite example."]
    [:form {:action "/links" :method "post"}
     [:input {:type "hidden" :name "__anti-forgery-token"
              :value (:anti-forgery-token request)}]
     [:p [:label "Title " [:input {:name "title"
                                   :maxlength max-title-length
                                   :required true}]]]
     [:p [:label "URL " [:input {:name "url"
                                 :type "url"
                                 :maxlength max-url-length
                                 :required true}]]]
     [:button {:type "submit"} "Save link"]]
    [:hr]
    (if (seq links)
      [:ul
       (for [{:link/keys [id title url]} links]
         [:li {:key (str id)} [:a {:href url :rel "noreferrer"} title]])]
      [:p "Nothing saved yet."])]])

(defn create-link-state [{:keys [params biff.fx/now]}]
  (let [title (normalize-title (request-param params :title))
        url (normalize-http-url (request-param params :url))]
    (if (and title url)
      {:write
       [:biff.sqlite.fx/authorized-write
        {:insert-into :link
         :values [{:link/id (random-uuid)
                   :link/title title
                   :link/url url
                   :link/created-at now}]
         :on-conflict [:link/url]
         :do-update-set [:link/title :link/created-at]}]
       :biff.fx/return {:status 303 :headers {"location" "/"}}}
      {:biff.fx/return
       {:status 400
        :headers {"content-type" "text/plain; charset=utf-8"}
        :body (str "title must be 1-" max-title-length
                   " characters and URL must be an absolute http(s) URL")}})))

(defroute home "/"
  [:biff.sqlite.fx/execute (links-query)]

  :get
  (fn [request links]
    (page request links)))

(defroute create-link "/links"
  :post
  create-link-state)

(def module
  {:biff.core/init (fn [_modules-var]
                     {:biff.sqlite/authorize #'authorize-local-write})
   :biff.sqlite/columns columns
   :biff.ring/routes [["" home create-link]]})

(def modules
  [(biff.core/module)
   (biff.ring/module)
   (biff.fx/module)
   (biff.sqlite/module)
   module])

(def components
  [biff.sqlite/use-sqlite
   biff.ring/use-jetty])

(defn config []
  {:biff.ring/host (or (System/getenv "HOST") "127.0.0.1")
   :biff.ring/port (parse-long (or (System/getenv "PORT") "8080"))
   :biff.ring/secure false
   ;; systemd creates this persistent secret before starting the JVM. Biff's
   ;; secret-delay prevents accidental disclosure through printed config.
   :biff.ring/cookie-secret (biff.core/secret-delay
                             (read-cookie-secret
                              (System/getenv "COOKIE_SECRET_FILE")))
   :biff.sqlite/db-path (or (System/getenv "SQLITE_DB_PATH") "storage/reading-list.db")
   :biff.sqlite/schema-path (or (System/getenv "SQLITE_SCHEMA_PATH") "storage/schema.sql")
   ;; Biff downloads sqldef when this differs from what the binary it finds
   ;; reports, so the NixOS unit sets SQLDEF_VERSION from the sqldef it puts on
   ;; PATH. The literal is only the fallback for a run outside that unit.
   :biff.sqlite/sqldef-version (or (System/getenv "SQLDEF_VERSION") "3.11.1")})

(defonce system (atom nil))

(defn start []
  (reset! system (biff.core/start (config) #'modules components)))

(defn stop []
  (when-some [ctx @system]
    (biff.core/stop ctx)
    (reset! system nil)))

(defonce shutdown-hook
  (Thread. stop "biff-reading-list-shutdown"))

(defn -main [& _args]
  (.addShutdownHook (Runtime/getRuntime) shutdown-hook)
  (start)
  @(promise))
