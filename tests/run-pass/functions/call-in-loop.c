// The accumulator and the counter are both live across the call.
//@ exit-code: 45

int add(int a, int b) {
    return a + b;
}

int main() {
    int s = 0;
    int i = 0;
    while (i < 10) {
        s = add(s, i);
        i = i + 1;
    }
    return s;
}
