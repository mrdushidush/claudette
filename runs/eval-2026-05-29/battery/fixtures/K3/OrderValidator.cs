namespace Shop
{
    // Validates purchase orders before they are submitted.
    public class OrderValidator
    {
        public bool ValidateOrder(Order order)
        {
            return order != null && order.Total > 0m && order.Lines.Count > 0;
        }
    }
}
