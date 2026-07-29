(ns com.example.todo-app.lib.email
  (:require [clojure.tools.logging :as log]
            [hato.client :as hato]))

(defn- send-mailersend
  [{:mailersend/keys [api-key from from-name reply-to]}
   {:keys [to subject html text]}]
  (let [response (hato/post
                  "https://api.mailersend.com/v1/email"
                  {:headers          {"Authorization" (str "Bearer " (force api-key))}
                   :content-type     :json
                   :throw-exceptions false
                   :as               :json
                   :form-params      {:from     {:email from
                                                 :name  from-name}
                                      :reply_to {:email reply-to
                                                 :name  from-name}
                                      :to       [{:email to}]
                                      :subject  subject
                                      :html     html
                                      :text     text}})]
    (when (<= 400 (:status response))
      (log/warn "MailerSend error:" (:body response)))
    (< (:status response) 400)))

(defn send-email [{:keys [mailersend/api-key]
                   :mailersend/keys [from reply-to]
                   :as ctx}
                  {:keys [to subject text html]}]
  (if api-key
    (if (every? (comp not-empty str) [from reply-to])
      (send-mailersend ctx {:to      to
                            :subject subject
                            :html    html
                            :text    text})
      (do
        (log/error "MAILERSEND_FROM and MAILERSEND_REPLY_TO are required when MAILERSEND_API_KEY is set")
        false))
    (do
      (println)
      (println "---")
      (println "To:     " to)
      (println "Subject:" subject)
      (println)
      (println text)
      (println "---")
      (println)
      true)))
