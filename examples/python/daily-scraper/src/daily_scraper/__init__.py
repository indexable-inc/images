from __future__ import annotations

import datetime as dt
import logging
from pathlib import Path

import duckdb
import httpx
from pydantic import BaseModel
from pydantic_settings import (
    BaseSettings,
    CliApp,
    CliSettingsSource,
    PydanticBaseSettingsSource,
    SettingsConfigDict,
)


LOGGER = logging.getLogger("daily_scraper")


class CliArgs(BaseSettings):
    model_config = SettingsConfigDict(
        cli_parse_args=True,
        cli_kebab_case=True,
        cli_prog_name="daily-scraper",
    )

    output_dir: Path
    repo: str = "indexable-inc/index"
    github_api_url: str = "https://api.github.com"
    user_agent: str = "ix-daily-scraper-example/0.1"

    @classmethod
    def settings_customise_sources(
        cls,
        settings_cls: type[BaseSettings],
        init_settings: PydanticBaseSettingsSource,
        env_settings: PydanticBaseSettingsSource,
        dotenv_settings: PydanticBaseSettingsSource,
        file_secret_settings: PydanticBaseSettingsSource,
    ) -> tuple[PydanticBaseSettingsSource, ...]:
        # Return only init + CLI sources; drop env/dotenv/secrets so that
        # ambient env vars (e.g. OUTPUT_DIR, REPO) cannot silently populate
        # settings (argparse parity, CWE-15).
        return (
            init_settings,
            CliSettingsSource(settings_cls, cli_parse_args=True),
        )


class GitHubRepoPayload(BaseModel):
    full_name: str
    default_branch: str
    stargazers_count: int
    forks_count: int
    open_issues_count: int


class RepoMetric(BaseModel):
    run_date: dt.date
    fetched_at: dt.datetime
    source_url: str
    repository: str
    default_branch: str
    stars: int
    forks: int
    open_issues: int


def fetch_repo_metric(repo: str, api_url: str, user_agent: str) -> RepoMetric:
    source_url = f"{api_url.rstrip('/')}/repos/{repo}"
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": user_agent,
    }

    with httpx.Client(headers=headers, follow_redirects=True, timeout=30.0) as client:
        response = client.get(source_url)
        _ = response.raise_for_status()
        payload = GitHubRepoPayload.model_validate(response.json())

    fetched_at = dt.datetime.now(dt.UTC).replace(microsecond=0)
    return RepoMetric(
        run_date=fetched_at.date(),
        fetched_at=fetched_at,
        source_url=source_url,
        repository=payload.full_name,
        default_branch=payload.default_branch,
        stars=payload.stargazers_count,
        forks=payload.forks_count,
        open_issues=payload.open_issues_count,
    )


def sql_string(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def write_parquet(metric: RepoMetric, output_dir: Path) -> Path:
    output_dir.mkdir(parents=True, exist_ok=True)
    output_path = output_dir / f"github-repo-metrics-{metric.run_date.isoformat()}.parquet"

    connection = duckdb.connect(database=":memory:")
    try:
        _ = connection.execute(
            """
            create table repo_metrics (
              run_date date,
              fetched_at timestamp with time zone,
              source_url varchar,
              repository varchar,
              default_branch varchar,
              stars integer,
              forks integer,
              open_issues integer
            )
            """,
        )
        _ = connection.execute(
            """
            insert into repo_metrics values (?, ?, ?, ?, ?, ?, ?, ?)
            """,
            [
                metric.run_date,
                metric.fetched_at,
                metric.source_url,
                metric.repository,
                metric.default_branch,
                metric.stars,
                metric.forks,
                metric.open_issues,
            ],
        )
        copy_sql = (
            f"copy repo_metrics to {sql_string(str(output_path))} (format parquet, compression zstd)"
        )
        _ = connection.execute(copy_sql)
    finally:
        connection.close()

    return output_path


def parse_args() -> CliArgs:
    return CliApp.run(CliArgs)


def main() -> None:
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
    args = parse_args()

    metric = fetch_repo_metric(
        repo=args.repo,
        api_url=args.github_api_url,
        user_agent=args.user_agent,
    )
    output_path = write_parquet(metric=metric, output_dir=args.output_dir)
    LOGGER.info("wrote %s", output_path)
