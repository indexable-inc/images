(ns com.example.todo-app.modules
  (:require [com.biffweb.background :as biff.background]
            [com.biffweb.core :as biff.core]
            [com.biffweb.datastar :as biff.datastar]
            [com.example.todo-app.app.admin :as admin]
            [com.example.todo-app.app.archive :as archive]
            [com.example.todo-app.app.auth :as auth]
            [com.example.todo-app.app.landing :as landing]
            [com.example.todo-app.app.todos :as todos]
            [com.example.todo-app.model.schema :as schema]
            [com.example.todo-app.model.tab-state :as model.tab-state]
            [com.example.todo-app.model.todo :as model.todo]
            [com.example.todo-app.model.user :as model.user]
            [com.biffweb.fx :as biff.fx]
            [com.biffweb.graph :as biff.graph]
            [com.biffweb.ring :as biff.ring]
            [com.biffweb.sqlite :as biff.sqlite]))

(def modules
  [(biff.core/module)
   (biff.ring/module)
   (biff.datastar/module)
   (biff.background/module)
   (biff.fx/module)
   (biff.graph/module)
   (biff.sqlite/module)
   model.user/module
   model.tab-state/module
   model.todo/module
   schema/module
   admin/module
   landing/module
   auth/module
   archive/module
   todos/module])
