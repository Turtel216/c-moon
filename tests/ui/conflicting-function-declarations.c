// Two declarations of one function that disagree about its parameter type.

int scale(int x);

int scale(long x) {
    return 1;
}

int main() {
    return scale(1);
}
