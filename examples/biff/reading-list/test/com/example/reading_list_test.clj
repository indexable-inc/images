(ns com.example.reading-list-test
  (:require [clojure.test :refer [deftest is run-tests testing]]
            [com.example.reading-list :as app]))

(deftest input-normalization
  (is (= "A link" (app/trim-to-nil "  A link  ")))
  (is (nil? (app/trim-to-nil " \t\n")))
  (is (nil? (app/trim-to-nil nil)))
  (let [max-title (apply str (repeat app/max-title-length "x"))]
    (is (= max-title (app/normalize-title max-title)))
    (is (nil? (app/normalize-title (str max-title "x")))))
  (is (nil? (app/normalize-title 42))))

(deftest query-contract
  (is (= {:select [:link/id :link/title :link/url :link/created-at]
          :from :link
          :order-by [[:link/created-at :desc]]}
         (app/links-query))))

(deftest url-validation
  (is (= "https://biffweb.com/p/biff2/"
         (app/normalize-http-url " https://biffweb.com/p/biff2/ ")))
  (is (= "http://localhost:8080/notes"
         (app/normalize-http-url "http://localhost:8080/notes")))
  (is (= "HTTPS://EXAMPLE.COM/path"
         (app/normalize-http-url "HTTPS://EXAMPLE.COM/path")))
  (doseq [url ["javascript:alert(1)"
               "ftp://example.com/archive"
               "https://user:password@example.com/"
               "https:/missing-host"
               "/relative/path"
               "not a URL"]]
    (is (nil? (app/normalize-http-url url)) url))
  (is (nil? (app/normalize-http-url 42)))
  (is (nil? (app/normalize-http-url
             (str "https://example.com/"
                  (apply str (repeat app/max-url-length "x")))))))

(deftest create-link-contract
  (let [id (parse-uuid "f7d7c901-ecaa-4f96-855a-298013d573e2")
        now #inst "2026-07-28T00:00:00.000-00:00"]
    (with-redefs [clojure.core/random-uuid (constantly id)]
      (is (= {:write
              [:biff.sqlite.fx/authorized-write
               {:insert-into :link
                :values [{:link/id id
                          :link/title "Biff 2"
                          :link/url "https://biffweb.com/p/biff2/"
                          :link/created-at now}]
                :on-conflict [:link/url]
                :do-update-set [:link/title :link/created-at]}]
              :biff.fx/return {:status 303 :headers {"location" "/"}}}
             (app/create-link-state
              {:params {"title" " Biff 2 "
                        "url" "https://biffweb.com/p/biff2/"}
               :biff.fx/now now})))))

  (testing "unsafe URLs and oversized titles are rejected before a write effect"
    (doseq [params [{:title "" :url "https://example.com"}
                    {:title "Bad" :url "javascript:alert(1)"}
                    {:title "Bad" :url "/relative"}
                    {:title "Bad" :url "https://user:password@example.com"}
                    {:title (apply str (repeat 201 "x"))
                     :url "https://example.com"}]]
      (let [result (app/create-link-state {:params params :biff.fx/now (java.util.Date.)})]
        (is (= 400 (get-in result [:biff.fx/return :status])))
        (is (not (contains? result :write)))))))

(deftest cookie-secret-contract
  (with-redefs [clojure.core/slurp (constantly "  generated-secret\n")]
    (is (= "generated-secret" (app/read-cookie-secret "/run/example-secret"))))
  (is (thrown-with-msg? clojure.lang.ExceptionInfo
                        #"COOKIE_SECRET_FILE is required"
                        (app/read-cookie-secret nil)))
  (with-redefs [clojure.core/slurp (constantly " \n")]
    (is (thrown-with-msg? clojure.lang.ExceptionInfo
                          #"must contain a non-blank secret"
                          (app/read-cookie-secret "/run/blank-secret")))))

(defn -main [& _args]
  (let [{:keys [fail error]} (run-tests 'com.example.reading-list-test)]
    (when (pos? (+ fail error))
      (throw (ex-info "Biff reading-list tests failed" {:fail fail :error error})))))
