(module
  ;; Default shared-audio instrument: two detuned triangle voices with a slow
  ;; tremolo. Stateless by construction: every sample is a pure function of
  ;; the absolute shared frame, so peers render identical bits.
  ;;
  ;; controls[0] = frequency in Hz (220 when unset)
  ;; controls[1] = gain (0.15 when unset)
  (memory (export "memory") 2)
  (func (export "sa_abi_version") (result i32) (i32.const 1))
  (func (export "sa_channels") (result i32) (i32.const 1))
  (func (export "sa_controls_ptr") (result i32) (i32.const 0))
  (func (export "sa_out_ptr") (result i32) (i32.const 1024))

  ;; Triangle wave in -1..1 from a phase in cycles.
  (func $tri (param $phase f64) (result f64)
    (f64.sub
      (f64.mul (f64.const 2)
        (f64.abs
          (f64.sub
            (f64.mul (f64.const 2)
              (f64.sub (local.get $phase) (f64.floor (local.get $phase))))
            (f64.const 1))))
      (f64.const 1)))

  (func (export "sa_render") (param $start i64) (param $n i32) (param $sr i32)
    (local $i i32) (local $t f64) (local $freq f64) (local $gain f64) (local $s f64)
    (local.set $freq (f64.promote_f32 (f32.load (i32.const 0))))
    (if (f64.eq (local.get $freq) (f64.const 0))
      (then (local.set $freq (f64.const 220))))
    (local.set $gain (f64.promote_f32 (f32.load (i32.const 4))))
    (if (f64.eq (local.get $gain) (f64.const 0))
      (then (local.set $gain (f64.const 0.15))))
    (block $done
      (loop $loop
        (br_if $done (i32.ge_s (local.get $i) (local.get $n)))
        (local.set $t
          (f64.div
            (f64.convert_i64_s
              (i64.add (local.get $start) (i64.extend_i32_s (local.get $i))))
            (f64.convert_i32_s (local.get $sr))))
        (local.set $s
          (f64.mul
            (f64.add
              (call $tri (f64.mul (local.get $freq) (local.get $t)))
              (f64.mul (f64.const 0.5)
                (call $tri
                  (f64.mul (f64.mul (local.get $freq) (f64.const 1.4983))
                    (local.get $t)))))
            (f64.mul (local.get $gain)
              (f64.add (f64.const 0.75)
                (f64.mul (f64.const 0.25)
                  (call $tri (f64.mul (f64.const 0.25) (local.get $t))))))))
        (f32.store
          (i32.add (i32.const 1024) (i32.mul (local.get $i) (i32.const 4)))
          (f32.demote_f64 (local.get $s)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop))))
)
