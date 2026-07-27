// Eight values are live across the call but only five callee-saved registers
// exist, so the rest have to reach the stack.
//@ exit-code: 42

int f(int x) {
    return x * 2;
}

int main() {
    int a = 1; int b = 2; int c = 3; int d = 4;
    int e = 5; int g = 6; int h = 7; int i = 8;
    int r = f(3);
    return a + b + c + d + e + g + h + i + r;
}
