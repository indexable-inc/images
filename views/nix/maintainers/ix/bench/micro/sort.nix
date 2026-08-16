# comparator-driven sort of 3k pseudo-random ints (rust arm insertion sort is O(n^2): keep n modest)
builtins.length (
  builtins.sort builtins.lessThan (
    builtins.genList (i: (i * 2654435761) - ((i * 2654435761) / 1000) * 1000) 3000
  )
)
