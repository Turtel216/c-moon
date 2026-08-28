// The C library is linked in already, so declaring one of its functions is
// all it takes to call it.
//@ exit-code: 42

extern int abs(int x);

int main() {
    return abs(-42);
}
