// The seventh and eighth arguments go on the stack, so a call into another
// translation unit only works if this compiler lays them out where the System
// V ABI says GCC-compiled code will look for them.
//@ aux-build: arithmetic.c
//@ exit-code: 36

extern int sum8(int a, int b, int c, int d, int e, int f, int g, int h);

int main() {
    return sum8(1, 2, 3, 4, 5, 6, 7, 8);
}
