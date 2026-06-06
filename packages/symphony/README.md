<p align="center">
  <img src="assets/logo.svg" width="80" alt="Symphony" />
</p>

# symphony

> [!IMPORTANT]
> Symphony is highly experimental software. Use it at your own risk: it can spawn Codex sessions, create branches, open PRs, and mutate Linear/GitHub state when credentials allow it.

The goal with this project is to create a multiplayer runtime for a fast-moving engineering team to be able to work in unison, similar to ZED's collaboration systems, and to be able to build a beautiful multiplayer experience for managing teams of agents at horizontal scale across various hosts.
It is broken up into multiple parts

room-server : codex-app-server wrapper runs on agent's host (rust)

room.ix.dev + tauri app : ui that teams spend time on (rust + svelete + tauri)

symphony : boring dag runtime for deteminsitic agent workflows (elixir)

<img alt="image" src="https://github.com/user-attachments/assets/eb06f062-3b2d-41a4-a679-94c5c2f847aa" />
