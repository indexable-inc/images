(ns com.example.todo-app.app.admin
  (:require [com.biffweb.admin :as biff.admin]
            [com.example.todo-app.lib.email :as lib.email]
            [com.biffweb.sqlite :as biff.sqlite]))

(defn- get-users [ctx]
  (->> (biff.sqlite/execute ctx {:select   [[:user/id :user-id]
                                            [:user/email :email]
                                            [:user/joined-at :joined-at]]
                                 :from     :user
                                 :order-by [[:user/joined-at :desc]]})
       vec))

(defn- todo-events [ctx column]
  (->> (biff.sqlite/execute
        ctx
        (cond-> {:select [[:todo/user-id :user-id]
                          [column :instant]]
                 :from   :todo}
          (= column :todo/updated-at)
          (assoc :where [:<> :todo/updated-at :todo/created-at])))
       (mapv (fn [{:keys [user-id instant]}]
               {:user-id user-id
                :instant instant}))))

(defn- signup-events [ctx]
  (->> (biff.sqlite/execute ctx {:select [[:user/id :user-id]
                                          [:user/joined-at :instant]]
                                 :from   :user})
       (mapv (fn [{:keys [user-id instant]}]
               {:user-id user-id
                :instant instant}))))

(defn- get-user-events [ctx]
  (vec (concat (signup-events ctx)
               (todo-events ctx :todo/created-at)
               (todo-events ctx :todo/updated-at))))

(def module
  (let [admin-module
        (biff.admin/module
         {:biff.admin/get-user-events #'get-user-events
          :biff.admin/get-users       #'get-users
          :biff.admin/send-email      #'lib.email/send-email})
        admin-init (:biff.core/init admin-module)]
    (assoc admin-module
           :biff.core/init
           (fn [modules-var]
             ;; `biff.admin/use-alerts` is a component, so it receives only
             ;; the initialized system map. Route middleware gets the params
             ;; above later and cannot supply the error handler's mailer.
             (assoc (admin-init modules-var)
                    :biff.admin/send-email #'lib.email/send-email)))))
