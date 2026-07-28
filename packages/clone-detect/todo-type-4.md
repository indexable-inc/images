# Type-4 Clone Detection (Semantic Clones)

Type-4 clones are semantically equivalent code fragments that achieve the same functionality through different implementations. These cannot be detected through AST structure alone and require ML-based approaches.

## Research Papers

### CodeBERT

- **Paper**: [CodeBERT for Code Clone Detection: A Replication Study](https://www.semanticscholar.org/paper/CodeBERT-for-Code-Clone-Detection:-A-Replication-Arshad-Abid/89a2f0c275823b4c968bfa656b8576743288807e)
- **Performance**: 96% recall on Type-4 clones
- **Approach**: Pre-trained transformer model that embeds code into vector space
- **Limitation**: Recall drops 15-40% on unseen functionalities

### CCStokener

- **Paper**: [Fast yet accurate code clone detection with semantic token](https://www.sciencedirect.com/science/article/abs/pii/S0164121223000134)
- **Performance**:
  - Near 100% recall on Type-1 and Type-2
  - Best recall on Type-3 and Type-4 compared to state-of-the-art
- **Approach**: Semantic token representation combined with efficient matching

## Implementation Considerations

### Embedding-Based Detection

1. **Code Embedding**: Convert code to vector representations using models like:
   - CodeBERT
   - GraphCodeBERT
   - UniXcoder
   - CodeSage V2

2. **Similarity Search**: Use approximate nearest neighbor search:
   - FAISS
   - Annoy
   - ScaNN

3. **Threshold Tuning**: Type-4 detection requires careful threshold selection to balance precision/recall

### Challenges

- **Training Data**: Need large corpus of labeled semantic clones
- **Cross-Language**: Harder to detect semantic clones across languages
- **Scalability**: Embedding and comparison can be expensive at scale
- **False Positives**: Common patterns (getters, setters) may be falsely flagged

## Potential Architecture

```
┌─────────────────────────────────────────────────┐
│                   clone-embed                    │
│  (Code → Vector embeddings via ML model)        │
├─────────────────────────────────────────────────┤
│  - Load pre-trained CodeBERT/similar model      │
│  - Embed significant code blocks                │
│  - Cache embeddings for incremental detection   │
└─────────────────────────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────┐
│                  clone-index                     │
│  (Vector similarity search)                     │
├─────────────────────────────────────────────────┤
│  - Build FAISS/similar index                    │
│  - Query for similar vectors                    │
│  - Return Type-4 candidates                     │
└─────────────────────────────────────────────────┘
```

## Dependencies to Consider

```toml
# For running ONNX models (CodeBERT export)
ort = "2.0"

# For vector similarity search
faiss = "0.12"  # or pure-Rust alternatives
```

## Related Work

- [Detecting Semantic Code Clones by Building AST-based Markov Chains Model](https://dl.acm.org/doi/10.1145/3551349.3560426) (ASE 2022)
- [Tritor: Detecting Semantic Code Clones by Building Social Networks](https://wu-yueming.github.io/Files/FSE2023_Tritor.pdf) (FSE 2023)
- [HAG: Hierarchical Attentive Graph Neural Network for Type-4 Detection](https://arxiv.org/html/2506.14470v1)
