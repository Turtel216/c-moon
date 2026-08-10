// Swapping two variables in a loop makes the loop header's phi nodes exchange
// their values. Today the assignment's own temporary breaks the cycle before
// out-of-SSA translation sees it; once copy propagation collapses that, this
// becomes the swap case that needs a temporary to lower. Three swaps of
// (1, 2) end at (2, 1).
//@ exit-code: 21

int main() {
    int a = 1;
    int b = 2;
    int i = 0;
    while (i < 3) {
        int t = a;
        a = b;
        b = t;
        i = i + 1;
    }
    return a * 10 + b;
}
