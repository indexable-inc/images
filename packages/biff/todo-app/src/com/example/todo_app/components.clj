(ns com.example.todo-app.components
  (:require [com.biffweb.admin :as biff.admin]
            [com.biffweb.background :as biff.background]
            [com.biffweb.config :as biff.config]
            [com.biffweb.ring :as biff.ring]
            [com.biffweb.sqlite :as biff.sqlite]))

(def components
  [biff.config/use-aero-config
   biff.admin/use-alerts
   biff.sqlite/use-sqlite
   biff.background/use-queues
   biff.ring/use-jetty])
