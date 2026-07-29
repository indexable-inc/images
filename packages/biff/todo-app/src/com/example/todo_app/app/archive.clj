(ns com.example.todo-app.app.archive
  (:require [com.example.todo-app.lib.middleware :as mid]
            [com.example.todo-app.lib.ui :as ui]
            [com.biffweb.fx :as biff.fx]
            [com.biffweb.ring :refer [defroute]]
            [com.biffweb.sqlite :as biff.sqlite])
  (:import [java.time Instant]))

(def queue-id :todo/archive)

(def queue-archive-job-states
  {:start
   (fn [{{user-id :uid} :session}]
     {:todo-rows    [:biff.sqlite.fx/execute
                     {:select   [:todo/id]
                      :from     :todo
                      :where    [:and
                                 [:= :todo/user-id user-id]
                                 [:= :todo/archived false]]
                      :order-by [[:todo/created-at :asc]
                                 [:todo/id :asc]]}]
      :todo-user-id user-id
      :biff.fx/next :submit})

   :submit
   (fn [{:keys [todo-rows todo-user-id]}]
     (let [jobs (->> todo-rows
                     (mapv :todo/id)
                     (partition-all 3)
                     (mapv (fn [todo-ids]
                             {:todo/archive-ids (vec todo-ids)
                              :todo/user-id     todo-user-id})))]
       (if (seq jobs)
         {:archive-jobs           [:biff.background/submit-jobs queue-id jobs]
          :todo.archive/batches   (count jobs)
          :todo.archive/submitted (reduce + (map #(count (:todo/archive-ids %))
                                                 jobs))}
         {:todo.archive/batches   0
          :todo.archive/submitted 0})))})

(def queue-archive-jobs!
  (biff.fx/machine
   ::queue-archive-jobs
   :start (:start queue-archive-job-states)
   :submit (:submit queue-archive-job-states)))

(defn archive-batch!
  [{:keys [biff.background/job] :as ctx}]
  (let [todo-ids (:todo/archive-ids job)
        user-id  (:todo/user-id job)]
    (when (seq todo-ids)
      (let [now (Instant/now)]
        (biff.sqlite/execute ctx {:update :todo
                                  :set    {:todo/archived    true
                                           :todo/archived-at now
                                           :todo/updated-at  now}
                                  :where  [:and
                                           [:= :todo/user-id user-id]
                                           [:in :todo/id todo-ids]
                                           [:= :todo/archived false]]})))))

(defroute archive-now-route "/app/archive"
  :post
  (fn [req]
    (merge ((:start queue-archive-job-states) req)
           {:biff.fx/next :archive-now-submit}))

  :archive-now-submit
  (fn [ctx]
    (merge ((:submit queue-archive-job-states) ctx)
           {:biff.fx/return (ui/no-content)})))

(def module
  {:biff.background/queues
   {queue-id {:n-threads 1
              :consumer  archive-batch!}}

   :biff.ring/routes
   [["" {:middleware [mid/wrap-signed-in]}
     archive-now-route]]})
