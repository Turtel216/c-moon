// The same expression written twice is computed once. (2 + 3) * (2 + 3) = 25.
//@ exit-code: 25

int f(int a, int b) {
    return (a + b) * (a + b);
}

int main() {
    return f(2, 3);
}
