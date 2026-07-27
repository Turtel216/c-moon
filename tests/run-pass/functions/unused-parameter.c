// `p3` is never read, so its live interval collapses and the allocator may
// hand its register to `p4`. Only the live parameter's copy may survive, or
// `p4` is destroyed before it is ever used.
//@ exit-code: 7

int pick(int p0, int p1, int p2, int p3, int p4, int p5, int p6) {
    int v0 = p6 - p0;
    if (p2 < p5) { p0 = p5 - 1; } else { p1 = p0 + 4; }
    return p4 * 1 + p1 + v0 * 0;
}

int main() {
    return pick(4, 3, 1, 5, 4, 2, 2);
}
