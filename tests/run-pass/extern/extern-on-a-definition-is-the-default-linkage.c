// `extern` on a definition says what a definition already says: the name has
// external linkage. It is accepted and the body is still emitted.
//@ exit-code: 12

extern int twice(int x) {
    return x + x;
}

int main() {
    return twice(6);
}
