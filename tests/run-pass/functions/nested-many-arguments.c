// Two eight-argument calls whose results are combined, so the first result is
// live across the second call.
//@ exit-code: 50

int inner(int a, int b, int c, int d, int e, int f, int g, int h) {
    return a - b + c - d + e - f + g - h;
}

int outer(int a, int b, int c, int d, int e, int f, int g, int h) {
    return inner(h, g, f, e, d, c, b, a) + inner(a, b, c, d, e, f, g, h);
}

int main() {
    return outer(9, 8, 7, 6, 5, 4, 3, 2) + 50;
}
