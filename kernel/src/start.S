.section .init, "ax", @progbits
.option push
.option norvc
.global _start
_start:
    j reset
.option pop

.section .vectors, "a", @progbits
.balign 4
.word 0, 0, trap, trap
/* Preserve the stock image's marker, not the different CH583 SDK marker. */
.word 0xf5f9bda9
.rept 31
.word trap
.endr

.section .text.reset, "ax", @progbits
.global reset
reset:
    csrci mstatus, 8
.option push
.option norelax
    la gp, __global_pointer$
    la sp, _stack_top
.option pop

    la a0, _vectors_load
    la a1, _vectors_start
    la a2, _vectors_end
1:
    bgeu a1, a2, 2f
    lw t0, 0(a0)
    sw t0, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 1b
2:
    la a0, _ramtext_load
    la a1, _ramtext_start
    la a2, _ramtext_end
8:
    bgeu a1, a2, 9f
    lw t0, 0(a0)
    sw t0, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 8b
9:
    fence.i
    la a0, _data_load
    la a1, _data_start
    la a2, _data_end
3:
    bgeu a1, a2, 4f
    lw t0, 0(a0)
    sw t0, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 3b
4:
    la a0, _bss_start
    la a1, _bss_end
5:
    bgeu a0, a1, 6f
    sw zero, 0(a0)
    addi a0, a0, 4
    j 5b
6:
    li t0, 0x1f
    csrw 0xbc0, t0
    /* Software trap frames require hardware stacking and nesting disabled. */
    csrw 0x804, zero
    la t0, _vectors_start
    ori t0, t0, 3
    csrw mtvec, t0
    tail rust_entry

.section .ramtext.trap, "ax", @progbits
.balign 4
.global trap
trap:
    csrci mstatus, 8
7:
    j 7b
