# ruff: noqa: ANN001, ANN003, ANN201, ANN204, PT009, PT018 -- dynamic unittest doubles

import asyncio
import importlib.util
import pathlib
import sys
import unittest


MODULE_PATH = pathlib.Path(__file__).parents[1] / "weave_slack_bot.py"
SPEC = importlib.util.spec_from_file_location("weave_slack_bot", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
bot_module = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = bot_module
SPEC.loader.exec_module(bot_module)


class FakeWeave:
    def __init__(self, records=()):
        self.records = list(records)
        self.seeded = []
        self.recorded = []
        self.dispatched = []
        self.replies = {}
        self.sent = []

    async def seed_agent(self, agent, model, system):
        self.seeded.append((agent, model, system))

    async def recorded_events(self):
        return self.records

    async def record(self, event):
        self.recorded.append(event)

    async def dispatch(self, event, agent):
        self.dispatched.append((event, agent))

    async def reply(self, event):
        return self.replies.get(event.key)

    async def mark_sent(self, event, reply_ts):
        self.sent.append((event, reply_ts))


class FakeSlack:
    def __init__(self):
        self.calls = []

    async def reactions_add(self, **kwargs):
        self.calls.append(("reactions_add", kwargs))
        return {"ok": True}

    async def reactions_remove(self, **kwargs):
        self.calls.append(("reactions_remove", kwargs))
        return {"ok": True}

    async def assistant_threads_setStatus(self, **kwargs):
        self.calls.append(("set_status", kwargs))
        return {"ok": True}

    async def chat_postMessage(self, **kwargs):
        self.calls.append(("chat_postMessage", kwargs))
        return {"ok": True, "ts": "200.1"}


class FakeSlackApiError(Exception):
    def __init__(self, code):
        super().__init__(code)
        self.response = {"error": code}


class FailingSlack(FakeSlack):
    def __init__(self, code):
        super().__init__()
        self.code = code

    async def chat_postMessage(self, **kwargs):
        self.calls.append(("chat_postMessage", kwargs))
        raise FakeSlackApiError(self.code)


def payload(event_type="app_mention", **event):
    body = {
        "type": event_type,
        "channel": "C1",
        "channel_type": "channel",
        "user": "U1",
        "text": "<@UBOT> help",
        "ts": "100.1",
    }
    body.update(event)
    return {"event_id": "Ev1", "event": body}


def event(ts="100.1"):
    return bot_module.SlackEvent(
        key=bot_module.event_key("C1", ts),
        event_id="Ev1",
        channel="C1",
        message_ts=ts,
        thread_ts=ts,
        user="U1",
        text="help",
    )


class SlackBotTests(unittest.IsolatedAsyncioTestCase):
    def make_bot(self, weave=None, slack=None):
        return bot_module.SlackBot(
            weave or FakeWeave(),
            slack or FakeSlack(),
            "slack-bot",
            "fable",
            "system",
            "UBOT",
        )

    def test_parse_accepts_mentions_dms_and_owned_thread_replies(self):
        bot = self.make_bot()
        mention = bot.parse(payload())
        self.assertIsNotNone(mention)
        self.assertEqual(mention.thread, ("C1", "100.1"))

        dm = bot.parse(
            payload(
                event_type="message",
                channel="D1",
                channel_type="im",
                text="hello",
            )
        )
        self.assertIsNotNone(dm)

        bot.owned_threads.add(("C1", "90.1"))
        reply = bot.parse(
            payload(
                event_type="message",
                thread_ts="90.1",
                ts="100.2",
                text="follow up",
            )
        )
        self.assertIsNotNone(reply)
        self.assertEqual(reply.thread_ts, "90.1")

    def test_parse_rejects_chatter_bots_edits_and_our_own_messages(self):
        bot = self.make_bot()
        self.assertIsNone(bot.parse(payload(event_type="message")))
        self.assertIsNone(bot.parse(payload(user="UBOT")))
        self.assertIsNone(bot.parse(payload(bot_id="B1", user="")))
        self.assertIsNone(bot.parse(payload(subtype="message_changed")))

    def test_app_mention_and_message_delivery_have_one_logical_key(self):
        bot = self.make_bot()
        mention = bot.parse(payload())
        message = bot.parse(payload(event_type="message", channel_type="im"))
        self.assertIsNotNone(mention)
        self.assertIsNotNone(message)
        self.assertEqual(mention.key, message.key)
        self.assertEqual(mention.message_id, message.message_id)

    async def test_ingress_is_durable_before_delivery_and_adapter_posts_reply(self):
        weave = FakeWeave()
        slack = FakeSlack()
        bot = self.make_bot(weave, slack)
        item = event()
        weave.replies[item.key] = "done"

        await bot.ingest(item)
        await bot.delivery_tasks[item.key]

        self.assertEqual(weave.recorded, [item])
        self.assertEqual(weave.dispatched, [(item, "slack-bot")])
        self.assertIn(item.thread, bot.owned_threads)
        self.assertEqual(weave.sent, [(item, "200.1")])
        post = next(call for call in slack.calls if call[0] == "chat_postMessage")
        self.assertEqual(post[1]["channel"], "C1")
        self.assertEqual(post[1]["thread_ts"], "100.1")
        self.assertEqual(post[1]["text"], "done")
        self.assertTrue(post[1]["client_msg_id"])

    async def test_start_recovers_pending_events_and_only_indexes_sent_ones(self):
        pending = event("100.1")
        sent = event("100.2")
        weave = FakeWeave(
            [
                bot_module.RecordedEvent(pending, "awaiting_reply"),
                bot_module.RecordedEvent(sent, "sent"),
            ]
        )
        weave.replies[pending.key] = "recovered"
        bot = self.make_bot(weave)

        await bot.start()
        await bot.delivery_tasks[pending.key]

        self.assertEqual(weave.seeded, [("slack-bot", "fable", "system")])
        self.assertEqual(bot.owned_threads, {pending.thread, sent.thread})
        self.assertEqual(weave.dispatched, [(pending, "slack-bot")])
        self.assertIn((pending, "200.1"), weave.sent)
        self.assertEqual(bot.sent_keys, {pending.key, sent.key})
        self.assertNotIn(sent.key, bot.delivery_tasks)

    async def test_duplicate_ingress_reuses_one_live_delivery_task(self):
        weave = FakeWeave()
        bot = self.make_bot(weave)
        item = event()

        await bot.ingest(item)
        first = bot.delivery_tasks[item.key]
        await bot.ingest(item)
        self.assertIs(first, bot.delivery_tasks[item.key])

        weave.replies[item.key] = "done"
        await asyncio.wait_for(first, timeout=2)

    async def test_sent_redelivery_is_recorded_but_never_dispatched_or_posted_again(
        self,
    ):
        weave = FakeWeave()
        slack = FakeSlack()
        bot = self.make_bot(weave, slack)
        item = event()
        bot.sent_keys.add(item.key)

        await bot.ingest(item)

        self.assertEqual(weave.recorded, [item])
        self.assertEqual(weave.dispatched, [])
        self.assertNotIn(item.key, bot.delivery_tasks)
        self.assertFalse(
            any(name == "chat_postMessage" for name, _kwargs in slack.calls)
        )

    async def test_permanent_slack_api_errors_do_not_retry_forever(self):
        weave = FakeWeave()
        slack = FailingSlack("missing_scope")
        bot = self.make_bot(weave, slack)
        item = event()
        weave.replies[item.key] = "done"

        await bot.ingest(item)
        await asyncio.wait_for(bot.delivery_tasks[item.key], timeout=1)

        posts = [name for name, _kwargs in slack.calls if name == "chat_postMessage"]
        self.assertEqual(posts, ["chat_postMessage"])
        self.assertEqual(weave.sent, [])


class EncodingTests(unittest.TestCase):
    def test_event_key_and_message_id_are_stable_and_valid(self):
        item = event()
        self.assertEqual(item.key, bot_module.event_key("C1", "100.1"))
        self.assertRegex(item.message_id, r"^msg:slack:[a-f0-9]{32}$")

    def test_fact_uses_weave_tagged_wire_shape(self):
        self.assertEqual(
            bot_module.fact("e", "a", "v"),
            {
                "fact": {
                    "entity": {"t": "str", "v": "e"},
                    "attr": "a",
                    "value": {"t": "str", "v": "v"},
                }
            },
        )


if __name__ == "__main__":
    unittest.main()
