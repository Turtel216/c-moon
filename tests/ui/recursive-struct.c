// A struct cannot contain itself: its own layout is not finished while its
// members are being laid out, so the member has no size.

struct Node {
    int value;
    struct Node inner;
};

int main() {
    return 0;
}
