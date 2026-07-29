#!/usr/bin/env python3
"""Exercise Todo App's real Datastar behavior in headless Firefox."""

from collections.abc import Iterator
from pathlib import Path
from urllib.parse import urlparse

from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.firefox.service import Service
from selenium.webdriver.remote.webdriver import WebDriver
from selenium.webdriver.remote.webelement import WebElement
from selenium.webdriver.support import expected_conditions as conditions
from selenium.webdriver.support.ui import WebDriverWait


BASE_URL = "http://localhost:8080"
COOKIE_JAR = Path("/tmp/cookies")  # noqa: S108 -- disposable test VM
SCREENSHOT = Path("/tmp/biff-todo-app.png")  # noqa: S108 -- disposable test VM
TODO_TITLE = "Created through real Datastar"


def cookies_from_netscape(path: Path) -> Iterator[dict[str, str | bool]]:
    for raw_line in path.read_text().splitlines():
        line = raw_line
        if line.startswith("#HttpOnly_"):
            line = line.removeprefix("#HttpOnly_")
        elif not line or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) != 7:
            continue
        yield {
            "name": fields[5],
            "value": fields[6],
            "path": fields[2] or "/",
            "secure": fields[3] == "TRUE",
        }


def todo(driver: WebDriver, title: str) -> WebElement:
    return driver.find_element(By.XPATH, f"//p[normalize-space()={title!r}]")


def main() -> None:
    options = Options()
    options.add_argument("-headless")
    service = Service(
        executable_path="/run/current-system/sw/bin/geckodriver",
        log_output="/tmp/geckodriver.log",  # noqa: S108 -- disposable test VM
    )
    driver = webdriver.Firefox(service=service, options=options)
    wait = WebDriverWait(driver, 20)

    try:
        driver.set_window_size(1280, 900)
        driver.get(BASE_URL)
        for cookie in cookies_from_netscape(COOKIE_JAR):
            driver.add_cookie(cookie)
        driver.get(f"{BASE_URL}/app")
        wait.until(conditions.title_is("Todo App"))
        wait.until(conditions.presence_of_element_located((By.NAME, "newtodo")))

        for element in driver.find_elements(By.CSS_SELECTOR, "script[src], link[href]"):
            url = element.get_attribute("src") or element.get_attribute("href")
            parsed = urlparse(url)
            assert parsed.hostname in {None, "localhost"}, url

        first_tab = driver.current_window_handle
        driver.switch_to.new_window("tab")
        second_tab = driver.current_window_handle
        driver.get(f"{BASE_URL}/app")
        wait.until(conditions.presence_of_element_located((By.NAME, "newtodo")))

        driver.switch_to.window(first_tab)
        todo_input = driver.find_element(By.NAME, "newtodo")
        todo_input.send_keys(TODO_TITLE)
        todo_input.find_element(
            By.XPATH, "./ancestor::form//button[@type='submit']"
        ).click()
        wait.until(
            lambda current: (
                current.find_element(By.NAME, "newtodo").get_attribute("value") == ""
            )
        )
        assert urlparse(driver.current_url).path == "/app", driver.current_url

        driver.switch_to.window(second_tab)
        wait.until(lambda current: todo(current, TODO_TITLE))

        driver.switch_to.window(first_tab)
        wait.until(lambda current: todo(current, TODO_TITLE))
        item = todo(driver, TODO_TITLE)
        article = item.find_element(By.XPATH, "./ancestor::article")
        article.find_element(By.CSS_SELECTOR, 'input[type="checkbox"]').click()

        driver.switch_to.window(second_tab)
        wait.until(
            lambda current: (
                "line-through" in todo(current, TODO_TITLE).get_attribute("class")
            )
        )
        assert driver.save_screenshot(str(SCREENSHOT))
        assert SCREENSHOT.stat().st_size > 10_000
    finally:
        driver.quit()

    print("real Datastar two-tab update passed")


if __name__ == "__main__":
    main()
