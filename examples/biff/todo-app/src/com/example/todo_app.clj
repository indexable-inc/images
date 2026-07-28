(ns com.example.todo-app
  (:require [clojure.string :as str]
            [com.biffweb.core :as biff.core]
            [com.example.todo-app.components :refer [components]]
            [com.example.todo-app.modules :refer [modules]])
  (:gen-class))

(defonce system (atom {}))

(defn read-cookie-secret [path]
  (when-not (some? path)
    (throw (ex-info "COOKIE_SECRET_FILE is required" {})))
  (or (not-empty (str/trim (slurp path)))
      (throw (ex-info "COOKIE_SECRET_FILE must contain a non-blank secret"
                      {:path path}))))

(defn configure-cookie-secret! []
  (when-some [path (System/getenv "COOKIE_SECRET_FILE")]
    (System/setProperty "biff.env.COOKIE_SECRET" (read-cookie-secret path))))

(defn start []
  (configure-cookie-secret!)
  (let [new-system (biff.core/start #'modules components)]
    (reset! system new-system)
    new-system))

(defn stop []
  (let [[old-system] (swap-vals! system (constantly {}))]
    (when (seq old-system)
      (biff.core/stop old-system)))
  :stopped)

(defn -main [& _args]
  (start)
  (.addShutdownHook (Runtime/getRuntime)
                    (Thread. ^Runnable #(stop) "todo-app-shutdown"))
  @(promise))
