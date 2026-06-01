# Network

Every VM has dual-stack IPv4 + IPv6 and up to 25 Gbps. Raw TCP, UDP, anything.

No port mapping, no load balancer, no ingress/egress fees, no enforced monthly cap. See [pricing](pricing.md#fair-use) for the soft thresholds where we'd want to talk before you push much further.

> [!NOTE]
> Our bare-metal provider includes automatic DDoS mitigation at the network edge. No configuration needed on your side.

## Public reachability

VMs bind ports directly. No forwarding layer.

```bash
ix vm exec vm-7f3a -- python3 -m http.server 8080
```

Reachable at the VM's public IP on 8080 immediately.

## DNS

Each VM gets `<vm-id>.vm.ix.dev`, resolving to both A and AAAA records.

## Private networking

VMs in the same group share a private network. East-west traffic stays local.

## Inter-region

Dedicated private 50 Gbps L2 interconnect between `us-east` and `us-west`, off shared transit. Latency: ~60-70 ms RTT (ballpark; measured numbers still owed).

> [!NOTE]
> Still needed: named firewalls/security-groups, VPC peering across groups, BYO BGP, stable egress IPs. Some exists via the low-level API but isn't documented; ask if you need it.
