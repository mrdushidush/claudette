namespace Shop
{
    // Decoy: persists orders. Has Save/Load, not ValidateOrder.
    public class OrderRepository
    {
        public void Save(Order order) { /* ... */ }
        public Order Load(int id) { return null; }
    }
}
